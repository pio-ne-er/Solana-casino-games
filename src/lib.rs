// Polymarket Trending Index Trading Library
// Self-contained project with all necessary code

pub mod types;
pub mod config;
pub mod indicators;
pub mod strategies;
pub mod monitor;
pub mod api;
pub mod models;
pub mod simulation;
pub mod trading;

// Re-export commonly used types
pub use types::*;
pub use config::*;
pub use indicators::*;
pub use strategies::*;
pub use monitor::*;
pub use api::*;
pub use models::*;

// Global history.toml logger (mirrors polymarket-trading-bot design)
use std::fs::File;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

static HISTORY_FILE: OnceLock<Mutex<File>> = OnceLock::new();

/// Initialize the global history file writer (called by main.rs)
pub fn init_history_file(file: File) {
    // Ignore error if already initialized; this crate only has one main
    let _ = HISTORY_FILE.set(Mutex::new(file));
}

/// Write a message to history.toml (without extra prefixes).
/// Callers can still `println!` separately if they want terminal output.
pub fn log_to_history(message: &str) {
    // Append to history.toml if initialized
    if let Some(file_mutex) = HISTORY_FILE.get() {
        if let Ok(mut file) = file_mutex.lock() {
            let _ = write!(file, "{}", message);
            let _ = file.flush();
        }
    }
}

/// Log a structured trading/monitoring event to history.toml with timestamp
pub fn log_trading_event(event: &str) {
    use chrono::Utc;
    let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    log_to_history(&format!("[{}] {}\n", timestamp, event));
}

/// Macro to log to both stderr and history.toml (like println! but persisted)
#[macro_export]
macro_rules! log_println {
    ($($arg:tt)*) => {{
        let message = format!($($arg)*);
        $crate::log_to_history(&format!("{}\n", message));
    }};
}
