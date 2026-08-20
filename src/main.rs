//! The console binary: `garld`.
//!
//! Keeps the console subsystem on Windows so the CLI and the live terminal view
//! work when run from a shell. The graphical surfaces are reachable from here
//! too (`garld gui`), but a double-clicked GUI should use `garld-gui`, which is
//! built for the windowing subsystem and opens no console window.

fn main() -> std::process::ExitCode {
    garld::cli::main()
}
