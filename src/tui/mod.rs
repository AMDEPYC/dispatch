mod status;
mod throbbing;

pub use status::Status;
pub use throbbing::Throbbing;

use std::sync::{LazyLock, Mutex};

use crossterm::{cursor, execute};
use ratatui::DefaultTerminal;

pub static TERMINAL: LazyLock<Mutex<DefaultTerminal>> = LazyLock::new(|| ratatui::init().into());

/// Cleanup function to restore terminal state
pub fn cleanup() {
    ratatui::restore();
    let _ = execute!(std::io::stdout(), cursor::Show);
}
