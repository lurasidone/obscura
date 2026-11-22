//! Terminal lifecycle shared by every obscura screen.

pub mod archive;
mod screen;
pub mod widgets;

pub use screen::{ArchiveFocus, Screen};

use std::io::stdout;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};

type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;

/// Puts the terminal in raw/alternate-screen mode and restores it on drop,
/// including when a panic unwinds.
pub struct TerminalGuard {
    active: bool,
    mouse: bool,
    previous: Arc<Mutex<Option<PanicHook>>>,
}

impl TerminalGuard {
    /// Alternate screen without mouse reporting.
    pub fn enter() -> Result<Self> {
        Self::enter_impl(false)
    }

    /// Alternate screen with mouse reporting, for screens that use it.
    pub fn enter_with_mouse() -> Result<Self> {
        Self::enter_impl(true)
    }

    fn enter_impl(mouse: bool) -> Result<Self> {
        enable_raw_mode().context("enable raw mode")?;
        let mut out = stdout();
        let entered = if mouse {
            execute!(out, EnterAlternateScreen, EnableMouseCapture)
        } else {
            execute!(out, EnterAlternateScreen)
        };
        if let Err(error) = entered {
            let _ = disable_raw_mode();
            return Err(error).context("enter alternate screen");
        }

        let previous: Arc<Mutex<Option<PanicHook>>> =
            Arc::new(Mutex::new(Some(std::panic::take_hook())));
        let hook_slot = previous.clone();
        std::panic::set_hook(Box::new(move |info| {
            let _ = disable_raw_mode();
            if mouse {
                let _ = execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture);
            } else {
                let _ = execute!(stdout(), LeaveAlternateScreen);
            }
            if let Some(prev) = hook_slot.lock().ok().and_then(|mut slot| slot.take()) {
                prev(info);
            }
        }));

        Ok(Self {
            active: true,
            mouse,
            previous,
        })
    }

    pub fn restore(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        disable_raw_mode().ok();
        if self.mouse {
            execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture).ok();
        } else {
            execute!(stdout(), LeaveAlternateScreen).ok();
        }
        if let Some(prev) = self.previous.lock().ok().and_then(|mut slot| slot.take()) {
            std::panic::set_hook(prev);
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}
