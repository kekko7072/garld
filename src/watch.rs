//! The live terminal view: redraws a full frame on a fixed interval.
//!
//! Uses the alternate screen buffer so the user's scrollback survives, and
//! restores the terminal on Ctrl-C rather than leaving it on the alternate
//! buffer with a hidden cursor.

use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::dashboard::Source;
use crate::probe::Probe;
use crate::render::{self, ViewOptions};

const ENTER_ALT: &str = "\x1b[?1049h";
const LEAVE_ALT: &str = "\x1b[?1049l";
const HIDE_CURSOR: &str = "\x1b[?25l";
const SHOW_CURSOR: &str = "\x1b[?25h";
const HOME: &str = "\x1b[H";
const CLEAR_LINE: &str = "\x1b[K";
const CLEAR_BELOW: &str = "\x1b[J";

/// Runs the live view until Ctrl-C, or until `count` frames have been drawn.
///
/// `interval` is clamped up to the minimum sampling interval; below it, CPU
/// percentages would be meaningless.
pub fn run(
    source: &mut Source,
    opts: &mut ViewOptions,
    interval: Duration,
    count: Option<usize>,
) -> io::Result<()> {
    let interval = interval.max(Probe::min_interval());
    let interactive = std::io::IsTerminal::is_terminal(&io::stdout());

    let running = Arc::new(AtomicBool::new(true));
    {
        let running = Arc::clone(&running);
        // A failed handler isn't fatal: Ctrl-C then just kills the process,
        // which is the same outcome minus the tidy restore.
        let _ = ctrlc::set_handler(move || running.store(false, Ordering::SeqCst));
    }

    if interactive {
        print!("{ENTER_ALT}{HIDE_CURSOR}");
        io::stdout().flush()?;
    }

    let result = draw_loop(source, opts, interval, count, &running, interactive);

    if interactive {
        print!("{SHOW_CURSOR}{LEAVE_ALT}");
        let _ = io::stdout().flush();
    }
    result
}

fn draw_loop(
    source: &mut Source,
    opts: &mut ViewOptions,
    interval: Duration,
    count: Option<usize>,
    running: &AtomicBool,
    interactive: bool,
) -> io::Result<()> {
    let mut frames = 0usize;
    // An explicit --limit wins; otherwise the table grows to fill the window.
    let user_limit = opts.query.limit;

    while running.load(Ordering::SeqCst) {
        let started = Instant::now();

        // Re-read the terminal every frame so resizing takes effect live.
        let (width, height) = terminal_dimensions();
        opts.width = width;

        // The first sample needs the priming wait; later ones are already
        // spaced by `interval`, which is at least the minimum.
        let data = if frames == 0 {
            source.sample()
        } else {
            source.sample_now()
        };

        // Sized after sampling, since the runner blocks above the table vary
        // in height with how many runners exist and whether they're busy.
        opts.query.limit = user_limit.or_else(|| Some(rows_for_processes(height, opts, &data)));

        let mut frame = render::dashboard(&data, opts);
        if interactive {
            frame.push_str(&format!(
                "\n\x1b[2m{:.1}s refresh · ctrl-c to quit\x1b[0m\n",
                interval.as_secs_f32()
            ));
        }

        paint(&frame, interactive)?;
        frames += 1;

        if count.is_some_and(|limit| frames >= limit) {
            break;
        }

        // Subtract render time so the interval is the period, not the gap.
        let elapsed = started.elapsed();
        if let Some(remaining) = interval.checked_sub(elapsed) {
            // Wake often enough that Ctrl-C feels immediate on long intervals.
            let mut left = remaining;
            let slice = Duration::from_millis(100);
            while left > Duration::ZERO && running.load(Ordering::SeqCst) {
                let step = left.min(slice);
                std::thread::sleep(step);
                left -= step;
            }
        }
    }
    Ok(())
}

/// Writes a frame without clearing the screen first, which would flicker.
///
/// Each line is padded out with a clear-to-end-of-line, and the region below
/// the frame is cleared once, so a shrinking frame leaves no debris.
fn paint(frame: &str, interactive: bool) -> io::Result<()> {
    let stdout = io::stdout();
    let mut out = anstream::AutoStream::auto(stdout.lock());

    if !interactive {
        return write!(out, "{frame}").and_then(|()| out.flush());
    }

    let mut buffer = String::with_capacity(frame.len() + 256);
    buffer.push_str(HOME);
    for line in frame.lines() {
        buffer.push_str(line);
        buffer.push_str(CLEAR_LINE);
        buffer.push_str("\r\n");
    }
    buffer.push_str(CLEAR_BELOW);

    write!(out, "{buffer}")?;
    out.flush()
}

/// Terminal size, with a sane fallback when stdout isn't a terminal.
fn terminal_dimensions() -> (usize, usize) {
    match terminal_size::terminal_size() {
        Some((terminal_size::Width(w), terminal_size::Height(h))) => (w as usize, h as usize),
        None => (100, 40),
    }
}

/// How many process rows fit under the runner section.
fn rows_for_processes(
    height: usize,
    opts: &ViewOptions,
    data: &crate::dashboard::Dashboard,
) -> usize {
    // host header (2) + runners heading (1) + table heading (2) + footer (2)
    const CHROME: usize = 7;

    let runner_rows: usize = data
        .runners
        .runners
        .iter()
        .map(|runner| {
            // Name line, then either a job pair or a single status line.
            let body = if runner.current_job.is_some() { 2 } else { 1 };
            // One line per listed job, plus the pass-rate summary line.
            let history = if opts.history > 0 && !runner.recent_jobs.is_empty() {
                opts.history.min(runner.recent_jobs.len()) + 1
            } else {
                0
            };
            1 + body + history
        })
        .sum();
    // The empty-state message occupies one line where blocks would be.
    let runner_rows = runner_rows.max(1);

    height.saturating_sub(CHROME + runner_rows).clamp(5, 200)
}
