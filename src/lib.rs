//! garld — GitHub Action Runner Local Dashboard.
//!
//! Shows the self-hosted GitHub Actions runners installed on this machine, what
//! they're running right now, and what the host is spending on it. Everything is
//! read locally: the runners' own install directories, their `_diag` logs, and
//! the process table. No GitHub token, no network.
//!
//! The crate ships two binaries. `garld` is the console program — the CLI, the
//! live terminal view, and the graphical surfaces launched by name. `garld-gui`
//! is the same graphical surfaces built for the windowing subsystem, so
//! double-clicking it on Windows opens a window instead of a console.

pub mod cli;
pub mod dashboard;
pub mod format;
pub mod probe;
pub mod render;
pub mod runner;
pub mod watch;

#[cfg(feature = "gui")]
pub mod gui;
#[cfg(feature = "tray")]
pub mod tray;

/// The graphical surfaces, each of which owns an event loop.
#[cfg(any(feature = "gui", feature = "tray"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// The full dashboard window.
    Window,
    /// The compact always-on-top desktop widget.
    Widget,
    /// The menu-bar / notification-area item.
    Tray,
}

#[cfg(any(feature = "gui", feature = "tray"))]
impl Surface {
    /// The subcommand that opens this surface.
    pub fn subcommand(self) -> &'static str {
        match self {
            Self::Window => "gui",
            Self::Widget => "widget",
            Self::Tray => "tray",
        }
    }

    /// Parses a surface name, accepting the subcommand spellings.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "gui" | "window" | "dashboard" => Some(Self::Window),
            "widget" => Some(Self::Widget),
            "tray" | "menubar" | "menu-bar" => Some(Self::Tray),
            _ => None,
        }
    }
}

/// Re-launches this executable to open another surface.
///
/// The widget and the status item use this to open siblings. Each surface owns
/// an event loop, and a process can only run one, so they can't be windows of
/// the same process.
#[cfg(any(feature = "gui", feature = "tray"))]
pub fn spawn_self(surface: Surface) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = std::process::Command::new(exe)
        .arg(surface.subcommand())
        .spawn();
}
