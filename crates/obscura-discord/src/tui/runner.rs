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

use crate::client::DiscordClient;
use crate::config::Config;
use crate::events::PrunerEvent;
use crate::ratelimit::RateLimiter;

use super::action::Effect;
use super::state::AppState;
use super::views;

// Repaint often enough to keep spinners and toasts moving without busy-waiting.
const TICK: Duration = Duration::from_millis(120);
const EVENT_BUFFER: usize = 512;

pub async fn run(cfg: Config) -> Result<()> {
    let mut guard = TerminalGuard::enter_with_mouse()?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend).context("create terminal")?;
    terminal.clear().ok();

    let (tx, mut rx) = mpsc::channel::<PrunerEvent>(EVENT_BUFFER);
    let run_cancel = Arc::new(AtomicBool::new(false));
    let probe_cancel = Arc::new(AtomicBool::new(false));
    let limiter = Arc::new(RateLimiter::new(
        Duration::from_millis(2000),
        run_cancel.clone(),
    ));

    let mut state = AppState::new(cfg);

    let mut probe: Option<tokio::task::JoinHandle<()>> = None;
    let mut run_task: Option<tokio::task::JoinHandle<()>> = None;

    if !state.form.token.trim().is_empty() {
        state.connection = super::state::Connection::Connecting;
        state.targets.loading = true;
        let token = state.form.token.clone();
        let session_path = state.config.session_path.clone();
        let base_url = state.config.base_url.clone();
        let probe_tx = tx.clone();
        let cancel = probe_cancel.clone();
        probe = Some(tokio::spawn(async move {
            probe_account(token, session_path, base_url, probe_tx, cancel).await;
        }));
    }

    let mut input = EventStream::new();
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let outcome = loop {
        while let Ok(ev) = rx.try_recv() {
            state.apply_event(ev);
        }
        reap_finished(&mut state, &mut run_task).await;

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
    let finished = run_task.as_ref().is_some_and(|t| t.is_finished());
    if !finished {
        return;
    }
    if let Some(task) = run_task.take() {
        let _ = task.await;
        if state.run.is_running() {
            state.run.status = crate::tui::state::RunStatus::Completed;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply_effects(
    effects: Vec<Effect>,
    state: &mut AppState,
    tx: &mpsc::Sender<PrunerEvent>,
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
                let token = state.form.token.trim().to_string();
                if token.is_empty() {
                    state.apply_event(PrunerEvent::ProbeFailed {
                        message: "no Discord token configured".into(),
                    });
                    continue;
                }
                let session_path = state.config.session_path.clone();
                let base_url = state.config.base_url.clone();
                let tx = tx.clone();
                probe_cancel.store(false, Ordering::Relaxed);
                let cancel = probe_cancel.clone();
                *probe = Some(tokio::spawn(async move {
                    probe_account(token, session_path, base_url, tx, cancel).await;
                }));
            }
            Effect::StartRun(config) => {
                if run_task.as_ref().is_some_and(|task| !task.is_finished()) {
                    state.set_toast(
                        "a prune run is already active",
                        crate::events::LogLevel::Warning,
                    );
                    continue;
                }
                run_task.take();
                limiter.reset();
                run_cancel.store(false, Ordering::Relaxed);
                limiter.set_user_throttle(Duration::from_millis(config.throttle_ms));
                let token = state.form.token.trim().to_string();
                let base_url = state.config.base_url.clone();
                let session_path = state.config.session_path.clone();
                let tx = tx.clone();
                let cancel = run_cancel.clone();
                let limiter = limiter.clone();
                *run_task = Some(tokio::spawn(async move {
                    let client = match DiscordClient::new(
                        &token,
                        Some(&base_url),
                        Some(&session_path),
                        limiter.clone(),
                    )
                    .await
                    {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = tx
                                .send(PrunerEvent::Error {
                                    message: format!("connect failed: {e}"),
                                })
                                .await;
                            return;
                        }
                    };
                    start_pruner(client, config, limiter, tx, cancel).await;
                }));
            }
            Effect::CancelRun => run_cancel.store(true, Ordering::Relaxed),
            Effect::Yank(text) => {
                if let Err(e) = crate::clipboard::copy(&text) {
                    state.set_toast(format!("clipboard: {e}"), crate::events::LogLevel::Error);
                } else {
                    state.set_toast(
                        "copied record to clipboard",
                        crate::events::LogLevel::Success,
                    );
                }
            }
        }
    }
    Ok(())
}

pub(crate) async fn probe_account(
    token: String,
    session_path: String,
    base_url: String,
    tx: mpsc::Sender<PrunerEvent>,
    cancel: Arc<AtomicBool>,
) {
    let limiter = Arc::new(RateLimiter::new(Duration::from_millis(350), cancel.clone()));
    let client =
        match DiscordClient::new(&token, Some(&base_url), Some(&session_path), limiter).await {
            Ok(c) => c,
            Err(e) => {
                let _ = tx
                    .send(PrunerEvent::ProbeFailed {
                        message: format!("client login failed: {e}"),
                    })
                    .await;
                return;
            }
        };

    let username = client.username();

    let _ = tx.send(PrunerEvent::Identity { username }).await;

    match client.private_channels().await {
        Ok(threads) => {
            let _ = tx.send(PrunerEvent::DMsLoaded { channels: threads }).await;
        }
        Err(e) => {
            let _ = tx
                .send(PrunerEvent::DMsLoadFailed {
                    message: e.to_string(),
                })
                .await;
        }
    }
}

async fn start_pruner(
    client: DiscordClient,
    config: crate::pruner::PrunerConfig,
    rate_limiter: Arc<RateLimiter>,
    tx: mpsc::Sender<PrunerEvent>,
    cancel: Arc<AtomicBool>,
) {
    let stats = crate::pruner::run(client, config, rate_limiter, tx.clone(), cancel).await;
    let _ = tx.send(PrunerEvent::Done { stats }).await;
}
