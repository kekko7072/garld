//! The command-line surface: argument parsing and the text views.

use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};

use crate::dashboard::Source;
use crate::probe::{Query, SortKey, SortKeyOrDefault};
use crate::render::{self, ViewOptions};
use crate::watch;

#[derive(Parser)]
#[command(
    name = "garld",
    version,
    about = "GitHub Action Runner Local Dashboard — local runners, jobs and host metrics",
    long_about = None,
    args_conflicts_with_subcommands = true,
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    global: GlobalArgs,

    /// Options for the default (no subcommand) dashboard view.
    #[command(flatten)]
    view: ViewArgs,
}

#[derive(Args, Clone)]
struct GlobalArgs {
    /// Extra runner install directory to inspect. Repeatable.
    ///
    /// garld already finds runners from running processes and the usual
    /// locations; use this for installs somewhere unusual. The environment
    /// variable GARLD_RUNNER_DIRS takes a path-separated list instead.
    #[arg(long = "runner-dir", value_name = "PATH", global = true)]
    runner_dirs: Vec<PathBuf>,

    /// Never emit colour, even to a terminal.
    #[arg(long, global = true)]
    no_color: bool,
}

/// Flags shared by the views that show processes.
#[derive(Args, Clone)]
struct ViewArgs {
    /// Order processes by this column.
    #[arg(short = 's', long, value_name = "KEY", default_value = "cpu")]
    sort: SortKey,

    /// Show at most this many processes. 0 shows all.
    #[arg(short = 'n', long, value_name = "N", default_value_t = 15)]
    limit: usize,

    /// Keep only processes matching this text, in name, command line, exe path
    /// or exact pid.
    #[arg(short = 'f', long, value_name = "TEXT")]
    filter: Option<String>,

    /// Keep only processes owned by this user.
    #[arg(short = 'u', long, value_name = "NAME")]
    user: Option<String>,

    /// Reverse the sort order.
    #[arg(short = 'r', long)]
    reverse: bool,

    /// Show full command lines instead of process names.
    #[arg(short = 'w', long)]
    wide: bool,

    /// Show every process on the host, not just the runner's own.
    ///
    /// By default the process table is limited to the runner tree: the service
    /// wrapper, the listener, and each worker with everything it spawned.
    #[arg(short = 'a', long)]
    all_processes: bool,

    /// Hide the process table, leaving runners and host metrics.
    #[arg(long)]
    no_processes: bool,

    /// Finished jobs to list per runner. 0 hides job history.
    #[arg(long, value_name = "N", default_value_t = 4)]
    history: usize,

    /// Print everything as JSON instead. Ignores the display flags above.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum Command {
    /// One-shot dashboard: runners, current jobs, host metrics, processes.
    Status {
        #[command(flatten)]
        view: ViewArgs,
    },

    /// Live dashboard in the terminal, refreshed on an interval.
    Watch {
        /// Seconds between refreshes.
        #[arg(short = 'i', long, value_name = "SECS", default_value_t = 2.0)]
        interval: f32,

        /// Stop after this many frames instead of running until Ctrl-C.
        #[arg(long, value_name = "N")]
        count: Option<usize>,

        #[command(flatten)]
        view: ViewArgs,
    },

    /// Just the process table.
    Ps {
        #[command(flatten)]
        view: ViewArgs,
    },

    /// Just the runners and their jobs.
    Runners {
        #[command(flatten)]
        view: ViewArgs,
    },

    /// Host and runner detail, one field per line.
    Info,

    /// Open the dashboard window.
    #[cfg(feature = "gui")]
    Gui,

    /// Open the compact always-on-top desktop widget.
    #[cfg(feature = "gui")]
    Widget,

    /// Run in the menu bar (macOS) or notification area (Windows, Linux).
    #[cfg(feature = "tray")]
    Tray,
}

/// Parses arguments and runs the requested view. Returns the process exit code.
pub fn main() -> std::process::ExitCode {
    let cli = Cli::parse();

    if cli.global.no_color {
        anstream::ColorChoice::Never.write_global();
    }

    match run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            // A closed pipe (`garld | head`) is a normal way to stop, not a failure.
            if let Some(io) = error.downcast_ref::<std::io::Error>()
                && io.kind() == std::io::ErrorKind::BrokenPipe
            {
                return std::process::ExitCode::SUCCESS;
            }
            let _ = writeln!(std::io::stderr(), "garld: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

type Fallible = Result<(), Box<dyn std::error::Error>>;

fn run(cli: Cli) -> Fallible {
    let roots = cli.global.runner_dirs.clone();

    match cli.command {
        None => status(&cli.view, roots),
        Some(Command::Status { view }) => status(&view, roots),
        Some(Command::Ps { view }) => {
            let mut view = view;
            view.no_processes = false;
            print_one(&view, roots, Section::Processes)
        }
        Some(Command::Runners { view }) => print_one(&view, roots, Section::Runners),
        Some(Command::Info) => {
            let data = Source::new(roots).sample();
            emit(&render::info(&data))
        }
        Some(Command::Watch {
            interval,
            count,
            view,
        }) => {
            let mut source = Source::new(roots);
            if view.json {
                // A JSON stream has no notion of a frame; emit one document.
                let data = source.sample();
                return emit(&render::json(&data));
            }
            let mut opts = view_options(&view);
            watch::run(
                &mut source,
                &mut opts,
                Duration::from_secs_f32(interval.max(0.1)),
                count,
            )?;
            Ok(())
        }

        #[cfg(feature = "gui")]
        Some(Command::Gui) => crate::gui::run_window(roots).map_err(Into::into),
        #[cfg(feature = "gui")]
        Some(Command::Widget) => crate::gui::run_widget(roots).map_err(Into::into),
        #[cfg(feature = "tray")]
        Some(Command::Tray) => crate::tray::run(roots),
    }
}

/// Which part of the dashboard a subcommand prints.
enum Section {
    Processes,
    Runners,
}

fn status(view: &ViewArgs, roots: Vec<PathBuf>) -> Fallible {
    let data = Source::new(roots).sample();
    if view.json {
        return emit(&render::json(&data));
    }
    emit(&render::dashboard(&data, &view_options(view)))
}

fn print_one(view: &ViewArgs, roots: Vec<PathBuf>, section: Section) -> Fallible {
    let data = Source::new(roots).sample();
    if view.json {
        return emit(&render::json(&data));
    }
    let opts = view_options(view);
    let text = match section {
        Section::Processes => render::processes(&data, &opts),
        Section::Runners => render::runners(&data, &opts),
    };
    emit(&text)
}

fn view_options(view: &ViewArgs) -> ViewOptions {
    ViewOptions {
        width: terminal_width(),
        show_processes: !view.no_processes,
        wide: view.wide,
        history: view.history,
        runners_only: !view.all_processes,
        query: Query {
            filter: view.filter.clone(),
            user: view.user.clone(),
            sort: SortKeyOrDefault(view.sort),
            reverse: view.reverse,
            limit: (view.limit > 0).then_some(view.limit),
        },
    }
}

/// Terminal width, or a readable default when piped.
fn terminal_width() -> usize {
    terminal_size::terminal_size()
        .map(|(terminal_size::Width(w), _)| w as usize)
        .unwrap_or(100)
        .max(60)
}

fn emit(text: &str) -> Fallible {
    let stdout = std::io::stdout();
    let mut out = anstream::AutoStream::auto(stdout.lock());
    out.write_all(text.as_bytes())?;
    out.flush()?;
    Ok(())
}
