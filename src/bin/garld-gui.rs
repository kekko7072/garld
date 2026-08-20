//! The windowed binary: `garld-gui`.
//!
//! Identical to `garld gui`, but built for the Windows windowing subsystem so
//! double-clicking it opens the dashboard rather than a console window. This is
//! the executable that macOS `.app` bundles and Windows shortcuts point at.
//!
//! It takes an optional surface name — `gui` (the default), `widget` or `tray` —
//! so a bundled app can open its siblings through the same executable.
#![cfg_attr(feature = "gui", windows_subsystem = "windows")]

#[cfg(feature = "gui")]
fn main() -> std::process::ExitCode {
    use garld::Surface;

    let requested = std::env::args().nth(1);
    let surface = match requested.as_deref() {
        None => Surface::Window,
        Some(name) => match Surface::parse(name) {
            Some(surface) => surface,
            None => {
                // No console to complain into on Windows, so fall back to the
                // window rather than exiting silently on a typo.
                Surface::Window
            }
        },
    };

    // Runner directories can't be passed as flags here: this binary is launched
    // by a double-click as often as by a shell. GARLD_RUNNER_DIRS covers it.
    let roots = Vec::new();

    let result = match surface {
        Surface::Window => garld::gui::run_window(roots).map_err(|e| e.to_string()),
        Surface::Widget => garld::gui::run_widget(roots).map_err(|e| e.to_string()),
        #[cfg(feature = "tray")]
        Surface::Tray => garld::tray::run(roots).map_err(|e| e.to_string()),
        #[cfg(not(feature = "tray"))]
        Surface::Tray => garld::gui::run_window(roots).map_err(|e| e.to_string()),
    };

    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("garld-gui: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Without the `gui` feature there are no windows to open.
#[cfg(not(feature = "gui"))]
fn main() -> std::process::ExitCode {
    eprintln!("garld-gui: this build has no graphical support (built without the `gui` feature)");
    std::process::ExitCode::FAILURE
}
