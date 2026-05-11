//! Raw-mode alt-screen setup, teardown, and panic-safe restoration.
//!
//! The browser holds the terminal in raw mode + the alternate screen for the
//! whole session. If we exit via the normal path, [`leave`] restores it. If
//! the process panics, [`install_panic_hook`] still restores it — keeping the
//! user's shell usable after a crash.

use std::io;

use anyhow::Result;
use ratatui::crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

pub type Tui = Terminal<CrosstermBackend<io::Stdout>>;

/// Enable raw mode + enter the alternate screen and hand back a configured
/// `ratatui::Terminal`.
pub fn enter() -> Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Restore the terminal: leave the alternate screen, drop raw mode, show the
/// cursor. Mirror of [`enter`]. Idempotent — safe to call after a panic hook
/// has already run.
pub fn leave(mut terminal: Tui) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// Install a panic hook that restores the terminal before the default hook
/// prints the panic message. Without this, a panic would leave the user's shell
/// in raw mode on the alt screen.
pub fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original(info);
    }));
}
