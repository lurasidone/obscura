use std::io::stdout;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use obscura_core::tui::TerminalGuard;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{Event, EventStream, KeyEventKind};
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::client::{self, InstagramClient};
use crate::config::Config;
use crate::events::CleanerEvent;
use crate::ratelimit::RateLimiter;

use super::action::Effect;
use super::state::AppState;
use super::views;

// Repaint often enough to keep spinners and toasts moving without busy-waiting.
const TICK: Duration = Duration::from_millis(120);
const EVENT_BUFFER: usize = 512;

pub async fn run(cfg: Config) -> Result<()> {
    let mut guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend).context("create terminal")?;
    terminal.clear().ok();

    let (tx, mut rx) = mpsc::channel::<CleanerEvent>(EVENT_BUFFER);
    let run_cancel = Arc::new(AtomicBool::new(false));
    let probe_cancel = Arc::new(AtomicBool::new(false));
    let limiter = Arc::new(RateLimiter::new(
        Duration::from_millis(2000),
        run_cancel.clone(),
    ));

    let mut state = AppState::new(cfg);

    let mut probe: Option<tokio::task::JoinHandle<()>> = None;
    let mut run_task: Option<tokio::task::JoinHandle<()>> = None;

    if !state.form.sessionid.trim().is_empty() {
        state.connection = super::state::Connection::Connecting;
        state.targets.loading = true;
        let sessionid = state.form.sessionid.clone();
        let session_path = state.config.session_path.clone();
        let base_url = state.config.base_url.clone();
        let probe_tx = tx.clone();
        let cancel = probe_cancel.clone();
        probe = Some(tokio::spawn(async move {
            probe_account(sessionid, session_path, base_url, probe_tx, cancel).await;
        }));
    }

    let mut input = EventStream::new();
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let outcome = loop {
        // Check before draining; a finished task cannot enqueue more events.
        let run_finished = run_task.as_ref().is_some_and(|t| t.is_finished());
        while let Ok(ev) = rx.try_recv() {
            state.apply_event(ev);
        }
        if run_finished {
            reap_finished(&mut state, &mut run_task).await;
        }
        if state.run.is_running() {
            state
                .run
                .sync_rate_limits(limiter.hits(), limiter.cooldown_remaining());
        }

        let frame = terminal.draw(|f| views::render(f, &state));
        if let Err(e) = frame {
            break Err(anyhow::anyhow!("render failed: {e}"));
        }

        tokio::select! {
            biased;
            maybe_event = input.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        if let Some(action) = super::keys::map(state.input_mode(), key) {
                            if action == super::action::Action::Quit {
                                break Ok(());
                            }
                            let effects = state.apply_action(action);
                            if effects.iter().any(|effect| matches!(effect, Effect::Quit)) {
                                break Ok(());
                            }
                            if let Err(e) = apply_effects(
                                effects,
                                &mut state,
                                &tx,
                                &run_cancel,
                                &probe_cancel,
                                &limiter,
                                &mut probe,
                                &mut run_task,
                            )
                            .await
                            {
                                break Err(e);
                            }
                        }
                    }
                    Some(Ok(Event::Resize(_, _))) => {}
                    Some(Ok(_)) => {}
                    Some(Err(e)) => break Err(anyhow::anyhow!("input error: {e}")),
                    None => break Ok(()),
                }
            }
            Some(ev) = rx.recv() => {
                state.apply_event(ev);
                while let Ok(ev) = rx.try_recv() {
                    state.apply_event(ev);
                }
            }
            _ = ticker.tick() => {
                if let Some(toast) = &state.toast {
                    if toast.is_expired(std::time::Instant::now()) {
                        state.toast = None;
                    }
                }
            }
        }
    };

    run_cancel.store(true, Ordering::Relaxed);
    probe_cancel.store(true, Ordering::Relaxed);
    if let Some(task) = run_task {
        let _ = tokio::time::timeout(Duration::from_secs(3), task).await;
    }
    if let Some(task) = probe {
        task.abort();
    }
    terminal.show_cursor().ok();
    guard.restore();
    outcome
}

async fn reap_finished(state: &mut AppState, run_task: &mut Option<tokio::task::JoinHandle<()>>) {
    if let Some(task) = run_task.take() {
        let _ = task.await;
        state.run.engine_active = false;
        // A normal ending reports itself before its task finishes.
        if state.run.is_running() {
            state.apply_event(CleanerEvent::Error {
                message: "the run stopped unexpectedly".into(),
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply_effects(
    effects: Vec<Effect>,
    state: &mut AppState,
    tx: &mpsc::Sender<CleanerEvent>,
    run_cancel: &Arc<AtomicBool>,
    probe_cancel: &Arc<AtomicBool>,
    limiter: &Arc<RateLimiter>,
    probe: &mut Option<tokio::task::JoinHandle<()>>,
    run_task: &mut Option<tokio::task::JoinHandle<()>>,
) -> Result<()> {
    for effect in effects {
        match effect {
            Effect::Quit => return Ok(()),
            Effect::Probe | Effect::LoadTargets => {
                if let Some(task) = probe.take() {
                    task.abort();
                }
                let sessionid = state.form.sessionid.trim().to_string();
                if sessionid.is_empty() {
                    state.apply_event(CleanerEvent::ProbeFailed {
                        message: "no session ID set".into(),
                    });
                    continue;
                }
                let session_path = state.config.session_path.clone();
                let base_url = state.config.base_url.clone();
                let tx = tx.clone();
                probe_cancel.store(false, Ordering::Relaxed);
                let cancel = probe_cancel.clone();
                *probe = Some(tokio::spawn(async move {
                    probe_account(sessionid, session_path, base_url, tx, cancel).await;
                }));
            }
            Effect::StartRun(config) => {
                // The state starts a run only after the previous run's final event.
                if let Some(task) = run_task.take() {
                    let _ = task.await;
                }
                limiter.reset();
                run_cancel.store(false, Ordering::Relaxed);
                limiter.set_user_throttle(Duration::from_millis(config.throttle_ms));
                let sessionid = state.form.sessionid.trim().to_string();
                let base_url = state.config.base_url.clone();
                let session_path = state.config.session_path.clone();
                let tx = tx.clone();
                let cancel = run_cancel.clone();
                let limiter = limiter.clone();
                *run_task = Some(tokio::spawn(async move {
                    let client = match InstagramClient::new(
                        &sessionid,
                        Some(&base_url),
                        Some(&session_path),
                        limiter.clone(),
                    )
                    .await
                    {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = tx
                                .send(CleanerEvent::Error {
                                    message: format!("connection failed: {e}"),
                                })
                                .await;
                            return;
                        }
                    };
                    start_cleaner(client, config, limiter, tx, cancel).await;
                }));
            }
            Effect::CancelRun => run_cancel.store(true, Ordering::Relaxed),
            Effect::Yank(text) => {
                if let Err(e) = crate::clipboard::copy(&text) {
                    state.set_toast(format!("clipboard: {e}"), crate::events::LogLevel::Error);
                } else {
                    state.set_toast(
                        "record copied to clipboard",
                        crate::events::LogLevel::Success,
                    );
                }
            }
        }
    }
    Ok(())
}

pub(crate) async fn probe_account(
    sessionid: String,
    session_path: String,
    base_url: String,
    tx: mpsc::Sender<CleanerEvent>,
    cancel: Arc<AtomicBool>,
) {
    let limiter = Arc::new(RateLimiter::new(Duration::from_millis(350), cancel.clone()));
    let client =
        match InstagramClient::new(&sessionid, Some(&base_url), Some(&session_path), limiter).await
        {
            Ok(c) => c,
            Err(e) => {
                let _ = tx
                    .send(CleanerEvent::ProbeFailed {
                        message: format!("login failed: {e}"),
                    })
                    .await;
                return;
            }
        };

    let username = client.username();

    let _ = tx.send(CleanerEvent::Identity { username }).await;

    match client::list_all_threads(&client, 500).await {
        Ok(threads) => {
            let _ = tx.send(CleanerEvent::DMsLoaded { channels: threads }).await;
        }
        Err(e) => {
            let _ = tx
                .send(CleanerEvent::DMsLoadFailed {
                    message: e.to_string(),
                })
                .await;
        }
    }
}

async fn start_cleaner(
    client: InstagramClient,
    config: crate::cleaner::CleanerConfig,
    rate_limiter: Arc<RateLimiter>,
    tx: mpsc::Sender<CleanerEvent>,
    cancel: Arc<AtomicBool>,
) {
    let stats = crate::cleaner::run(client, config, rate_limiter, tx.clone(), cancel).await;
    let _ = tx.send(CleanerEvent::Done { stats }).await;
}
