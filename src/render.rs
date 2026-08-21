//! Terminal rendering. Every function returns a `String` rather than printing,
//! so the live view can compose a whole frame and emit it in one write.
//!
//! Styles are always emitted; `anstream` strips them when stdout isn't a
//! terminal and translates them on legacy Windows consoles.

use std::fmt::Write as _;

use anstyle::{AnsiColor, Color, Style};

use crate::dashboard::Dashboard;
use crate::format as fmt;
use crate::probe::{ProcInfo, Query, select};
use crate::runner::{Job, JobResult, PidRole, Runner, RunnerState};

fn fg(color: AnsiColor) -> Style {
    Style::new().fg_color(Some(Color::Ansi(color)))
}

fn bold() -> Style {
    Style::new().bold()
}

fn dim() -> Style {
    Style::new().dimmed()
}

fn heading() -> Style {
    Style::new()
        .bold()
        .fg_color(Some(Color::Ansi(AnsiColor::Cyan)))
}

/// Green under half load, yellow approaching saturation, red at it.
fn load_style(fraction: f32) -> Style {
    if fraction >= 0.9 {
        fg(AnsiColor::Red)
    } else if fraction >= 0.65 {
        fg(AnsiColor::Yellow)
    } else {
        fg(AnsiColor::Green)
    }
}

fn state_style(state: RunnerState) -> Style {
    match state {
        RunnerState::Busy => fg(AnsiColor::Green).bold(),
        RunnerState::Idle => fg(AnsiColor::Blue),
        RunnerState::Offline => fg(AnsiColor::Red),
    }
}

fn result_style(result: Option<JobResult>) -> Style {
    match result {
        Some(JobResult::Succeeded) => fg(AnsiColor::Green),
        Some(JobResult::Failed) => fg(AnsiColor::Red),
        Some(JobResult::Canceled) => fg(AnsiColor::Yellow),
        Some(JobResult::Abandoned) => fg(AnsiColor::Magenta),
        _ => dim(),
    }
}

fn role_style(role: PidRole) -> Style {
    match role {
        PidRole::Listener => fg(AnsiColor::Blue),
        PidRole::Worker => fg(AnsiColor::Green).bold(),
        PidRole::JobStep => fg(AnsiColor::Green),
        PidRole::Supervisor => dim(),
    }
}

/// What to include in a rendered view.
pub struct ViewOptions {
    pub width: usize,
    /// Include the process table.
    pub show_processes: bool,
    /// Show full command lines instead of process names.
    pub wide: bool,
    /// Filtering and ordering for the process table.
    pub query: Query,
    /// Finished jobs to list per runner. 0 hides the history line.
    pub history: usize,
    /// Limit the table to runner-owned processes. On by default; the CLI's
    /// `--all-processes` and the window's checkbox turn it off.
    pub runners_only: bool,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            width: 100,
            show_processes: true,
            wide: false,
            query: Query::default(),
            history: 4,
            runners_only: true,
        }
    }
}

/// The full dashboard: host header, runners, then processes.
pub fn dashboard(data: &Dashboard, opts: &ViewOptions) -> String {
    let mut out = String::new();
    out.push_str(&header(data, opts));
    out.push('\n');
    out.push_str(&runners(data, opts));
    if opts.show_processes {
        out.push('\n');
        out.push_str(&processes(data, opts));
    }
    out
}

/// Host identity and the machine-wide meters.
pub fn header(data: &Dashboard, opts: &ViewOptions) -> String {
    let host = &data.snapshot.host;
    let metrics = &data.snapshot.metrics;
    let mut out = String::new();

    let cores = match host.physical_cores {
        Some(physical) if physical != host.logical_cores => {
            format!("{} cores / {physical} physical", host.logical_cores)
        }
        _ => format!("{} cores", host.logical_cores),
    };

    let _ = writeln!(
        out,
        "{b}garld{b:#} {d}·{d:#} {name} {d}·{d:#} {os} {ver} {d}({arch}, {cores}){d:#} {d}·{d:#} up {up}",
        b = bold(),
        d = dim(),
        name = host.hostname,
        os = host.os,
        ver = host.os_version,
        arch = host.arch,
        up = fmt::duration(metrics.uptime_secs),
    );

    // Meter width scales with the terminal so narrow windows still line up.
    let meter_width = (opts.width / 6).clamp(6, 18);
    let cpu_fraction = metrics.cpu_percent / 100.0;
    let mem_fraction = metrics.mem_percent() / 100.0;

    let cpu_style = load_style(cpu_fraction);
    let mem_style = load_style(mem_fraction);

    let _ = write!(
        out,
        "{d}cpu{d:#} {cs}{cpu_meter}{cs:#} {cs}{cpu:>5}%{cs:#}   {d}mem{d:#} {ms}{mem_meter}{ms:#} {used}/{total} {d}({mem:.0}%){d:#}",
        d = dim(),
        cs = cpu_style,
        ms = mem_style,
        cpu_meter = fmt::meter(cpu_fraction, meter_width),
        mem_meter = fmt::meter(mem_fraction, meter_width),
        cpu = fmt::percent(metrics.cpu_percent),
        used = fmt::bytes(metrics.mem_used),
        total = fmt::bytes(metrics.mem_total),
        mem = metrics.mem_percent(),
    );

    if metrics.swap_total > 0 {
        let _ = write!(
            out,
            "   {d}swap{d:#} {}/{}",
            fmt::bytes(metrics.swap_used),
            fmt::bytes(metrics.swap_total),
            d = dim(),
        );
    }
    if metrics.has_load_avg() {
        let _ = write!(
            out,
            "   {d}load{d:#} {:.2} {:.2} {:.2}",
            metrics.load_avg[0],
            metrics.load_avg[1],
            metrics.load_avg[2],
            d = dim(),
        );
    }
    let _ = writeln!(out, "   {d}{} procs{d:#}", metrics.process_count, d = dim());

    out
}

/// The runner section: one block per install.
pub fn runners(data: &Dashboard, opts: &ViewOptions) -> String {
    let report = &data.runners;
    let mut out = String::new();

    let _ = writeln!(
        out,
        "{h}RUNNERS{h:#}  {g}{busy} busy{g:#} {d}·{d:#} {idle} idle {d}·{d:#} {off_style}{off} offline{off_style:#}",
        h = heading(),
        d = dim(),
        g = if report.busy() > 0 {
            fg(AnsiColor::Green).bold()
        } else {
            dim()
        },
        off_style = if report.offline() > 0 {
            fg(AnsiColor::Red)
        } else {
            dim()
        },
        busy = report.busy(),
        idle = report.idle(),
        off = report.offline(),
    );

    if report.runners.is_empty() {
        let _ = writeln!(
            out,
            "{d}  no runner installs found. Searched {} location(s); pass --runner-dir or set GARLD_RUNNER_DIRS.{d:#}",
            report.searched.len(),
            d = dim(),
        );
        return out;
    }

    for runner in &report.runners {
        out.push_str(&runner_block(runner, data.sampled_at, opts));
    }
    out
}

fn runner_block(runner: &Runner, now: i64, opts: &ViewOptions) -> String {
    let mut out = String::new();
    let style = state_style(runner.state);

    let bullet = match runner.state {
        RunnerState::Busy => '●',
        RunnerState::Idle => '○',
        RunnerState::Offline => '◌',
    };

    let mut tail = String::new();
    if let Some(scope) = runner.scope() {
        let _ = write!(tail, " {d}·{d:#} {scope}", d = dim());
    }
    if let Some(version) = &runner.version {
        let _ = write!(tail, " {d}· v{version}{d:#}", d = dim());
    }
    if let Some(pool) = runner.config.as_ref().and_then(|c| c.pool_name.as_deref()) {
        let _ = write!(tail, " {d}· pool {pool}{d:#}", d = dim());
    }

    let _ = writeln!(
        out,
        "{s}{bullet} {label:<7}{s:#} {b}{name}{b:#}{tail}",
        s = style,
        b = bold(),
        label = runner.state.label(),
        name = fmt::truncate(&runner.name(), 34),
    );

    match (&runner.current_job, runner.state) {
        (Some(job), _) => out.push_str(&active_job_line(runner, job, now, opts)),
        (None, RunnerState::Offline) => {
            let _ = writeln!(
                out,
                "  {d}not running{d:#}{}",
                runner
                    .service
                    .as_deref()
                    .map(|s| format!(" {d}· service {s}{d:#}", d = dim()))
                    .unwrap_or_default(),
                d = dim(),
            );
        }
        (None, _) => {
            // The listener's own cost is unreadable when garld can't open the
            // process, which is the norm for a Windows service runner. Say so
            // rather than printing zeroes it can't stand behind.
            let cost = match runner.listener_cost {
                Some(cost) => format!(
                    " {d}· up {up}{d:#} {d}· {cpu:.1}% cpu, {mem}{d:#}",
                    d = dim(),
                    up = fmt::duration(cost.uptime_secs),
                    cpu = cost.cpu_percent,
                    mem = fmt::bytes(cost.mem_bytes),
                ),
                None => format!(" {d}· cost unavailable (needs elevation){d:#}", d = dim()),
            };
            let _ = writeln!(
                out,
                "  {d}waiting for work{d:#} {d}· listener pid {pid}{d:#}{cost}",
                pid = runner
                    .listener_pid
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "-".into()),
                d = dim(),
            );
        }
    }

    if opts.history > 0 && !runner.recent_jobs.is_empty() {
        out.push_str(&history_block(runner, opts.history));
    }
    out
}

fn active_job_line(runner: &Runner, job: &Job, now: i64, _opts: &ViewOptions) -> String {
    let mut out = String::new();
    let elapsed = job.duration_secs(now);

    let _ = writeln!(
        out,
        "  {h}job{h:#}  {c}{repo}{c:#}{sep}{b}{name}{b:#} {d}·{d:#} {y}{elapsed}{y:#}",
        h = fg(AnsiColor::Green),
        b = bold(),
        c = fg(AnsiColor::Cyan),
        d = dim(),
        y = fg(AnsiColor::Yellow),
        repo = job.short_repo(runner.scope()).unwrap_or(""),
        sep = if job.repository.is_some() { " · " } else { "" },
        name = job.full_name(),
        elapsed = fmt::duration(elapsed),
    );

    let _ = writeln!(
        out,
        "  {d}cost{d:#} {cs}{cpu:.0}% cpu{cs:#} {d}·{d:#} {mem} {d}·{d:#} {procs} procs {d}· listener pid {pid} up {up}{d:#}",
        d = dim(),
        cs = load_style(runner.job_cpu / 100.0),
        cpu = runner.job_cpu,
        mem = fmt::bytes(runner.job_mem),
        procs = runner.job_proc_count(),
        pid = runner
            .listener_pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".into()),
        up = runner
            .listener_cost
            .map(|cost| fmt::duration(cost.uptime_secs))
            .unwrap_or_else(|| "?".into()),
    );
    out
}

/// Recent jobs, one per line.
///
/// A line each rather than one packed line: every entry now carries a
/// repository, and `vault · release / ios` next to `management · release / ios`
/// only reads if they're aligned.
fn history_block(runner: &Runner, limit: usize) -> String {
    let mut out = String::new();
    let scope = runner.scope();

    for (index, job) in runner.recent_jobs.iter().take(limit).enumerate() {
        let label = if index == 0 { "last" } else { "    " };
        let _ = writeln!(
            out,
            "  {d}{label}{d:#} {s}{glyph}{s:#} {name} {d}{dur}{d:#}",
            d = dim(),
            s = result_style(job.result),
            glyph = job.result.map(JobResult::glyph).unwrap_or('·'),
            name = fmt::pad_right(&fmt::truncate(&job.labelled(scope), 40), 40),
            dur = fmt::duration(job.duration_secs(job.finished.unwrap_or(job.started))),
        );
    }

    if let Some(rate) = runner.success_rate() {
        let _ = writeln!(
            out,
            "  {d}     {:.0}% pass over {} recorded job(s){d:#}",
            rate * 100.0,
            runner.recent_jobs.len(),
            d = dim(),
        );
    }
    out
}

/// The process table.
///
/// Columns that carry no data on this platform are dropped rather than filled
/// with dashes: macOS reports no per-process thread counts, and the ROLE column
/// is meaningless on a host with no runners.
pub fn processes(data: &Dashboard, opts: &ViewOptions) -> String {
    let roles = data.runners.pid_roles();
    let mut out = String::new();

    let all = &data.snapshot.processes;
    let pool: Vec<ProcInfo> = if opts.runners_only {
        all.iter()
            .filter(|p| roles.contains_key(&p.pid))
            .cloned()
            .collect()
    } else {
        all.clone()
    };

    let rows = select(&pool, &opts.query);
    let matched = {
        let mut unlimited = opts.query.clone();
        unlimited.limit = None;
        select(&pool, &unlimited).len()
    };

    let scope = if opts.runners_only { "runner " } else { "" };
    let shown = if rows.len() < matched {
        format!("top {} of {matched}", rows.len())
    } else {
        matched.to_string()
    };
    let _ = writeln!(
        out,
        "{h}PROCESSES{h:#}  {d}{shown} {scope}process(es), by {sort}{d:#}",
        h = heading(),
        d = dim(),
        sort = opts.query.sort.0,
    );

    let show_role = rows.iter().any(|p| roles.contains_key(&p.pid));
    let show_threads = rows.iter().any(|p| p.threads.is_some());

    const PID_W: usize = 7;
    const ROLE_W: usize = 8;
    const USER_W: usize = 12;
    const PCT_W: usize = 6;
    const RSS_W: usize = 8;
    const TIME_W: usize = 9;
    const THR_W: usize = 5;

    let fixed = PID_W
        + 1
        + if show_role { ROLE_W + 1 } else { 0 }
        + USER_W
        + 1
        + PCT_W
        + 1
        + PCT_W
        + 1
        + RSS_W
        + 1
        + TIME_W
        + if show_threads { THR_W } else { 0 }
        + 2;
    let name_width = opts.width.saturating_sub(fixed).max(18);

    let _ = writeln!(
        out,
        "{d}{pid} {role}{user} {cpu} {mem} {rss} {time}{thr}  {name}{d:#}",
        d = dim(),
        pid = fmt::pad_left("PID", PID_W),
        role = if show_role {
            format!("{} ", fmt::pad_right("ROLE", ROLE_W))
        } else {
            String::new()
        },
        user = fmt::pad_right("USER", USER_W),
        cpu = fmt::pad_left("CPU%", PCT_W),
        mem = fmt::pad_left("MEM%", PCT_W),
        rss = fmt::pad_left("RSS", RSS_W),
        time = fmt::pad_right("TIME", TIME_W),
        thr = if show_threads {
            fmt::pad_left("THR", THR_W)
        } else {
            String::new()
        },
        name = if opts.wide { "COMMAND" } else { "NAME" },
    );

    if rows.is_empty() {
        // An empty runner-scoped table is the normal state of an idle host, so
        // say why it's empty and how to widen it rather than just "no matches".
        let hint = if opts.runners_only && opts.query.filter.is_none() {
            "no runner processes running — pass -a to show every process"
        } else {
            "nothing matched"
        };
        let _ = writeln!(out, "{d}  {hint}{d:#}", d = dim());
        return out;
    }

    for proc in &rows {
        let (role_text, row_style) = match roles.get(&proc.pid) {
            Some((role, _)) => (role.label(), role_style(*role)),
            None => ("", Style::new()),
        };

        let role_cell = if show_role {
            format!(
                "{rs}{}{rs:#} ",
                fmt::pad_right(role_text, ROLE_W),
                rs = row_style
            )
        } else {
            String::new()
        };
        let thr_cell = if show_threads {
            fmt::pad_left(
                &proc
                    .threads
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "-".into()),
                THR_W,
            )
        } else {
            String::new()
        };

        let name = if opts.wide { &proc.command } else { &proc.name };

        let _ = writeln!(
            out,
            "{pid} {role}{user} {cs}{cpu}{cs:#} {mem} {rss} {time}{thr}  {ns}{name}{ns:#}",
            pid = fmt::pad_left(&proc.pid.to_string(), PID_W),
            role = role_cell,
            user = fmt::pad_right(
                &fmt::truncate(proc.user.as_deref().unwrap_or("-"), USER_W),
                USER_W
            ),
            cs = load_style(proc.cpu_percent / 100.0),
            cpu = fmt::pad_left(&fmt::percent(proc.cpu_percent), PCT_W),
            mem = fmt::pad_left(&fmt::percent(proc.mem_percent), PCT_W),
            rss = fmt::pad_left(&fmt::bytes(proc.mem_bytes), RSS_W),
            time = fmt::pad_right(&fmt::cpu_time(proc.cpu_time_ms), TIME_W),
            thr = thr_cell,
            ns = row_style,
            name = fmt::truncate(name, name_width),
        );
    }
    out
}

/// Host and metric detail, without processes or runners.
pub fn info(data: &Dashboard) -> String {
    let host = &data.snapshot.host;
    let metrics = &data.snapshot.metrics;
    let mut out = String::new();

    let mut row = |key: &str, value: String| {
        let _ = writeln!(out, "{d}{:<16}{d:#}{value}", key, d = dim());
    };

    row("hostname", host.hostname.clone());
    row("os", format!("{} {}", host.os, host.os_version));
    row("kernel", host.kernel.clone());
    row("arch", host.arch.clone());
    row(
        "cores",
        match host.physical_cores {
            Some(physical) => format!("{} logical, {physical} physical", host.logical_cores),
            None => format!("{} logical", host.logical_cores),
        },
    );
    row("uptime", fmt::duration(metrics.uptime_secs));
    row("cpu", format!("{:.1}%", metrics.cpu_percent));
    row(
        "memory",
        format!(
            "{} / {} ({:.0}%), {} available",
            fmt::bytes(metrics.mem_used),
            fmt::bytes(metrics.mem_total),
            metrics.mem_percent(),
            fmt::bytes(metrics.mem_available),
        ),
    );
    if metrics.swap_total > 0 {
        row(
            "swap",
            format!(
                "{} / {} ({:.0}%)",
                fmt::bytes(metrics.swap_used),
                fmt::bytes(metrics.swap_total),
                metrics.swap_percent(),
            ),
        );
    }
    if metrics.has_load_avg() {
        row(
            "load",
            format!(
                "{:.2} {:.2} {:.2}",
                metrics.load_avg[0], metrics.load_avg[1], metrics.load_avg[2]
            ),
        );
    }
    row("processes", metrics.process_count.to_string());
    if let Some(threads) = metrics.thread_count {
        row("threads", threads.to_string());
    }
    row(
        "runners",
        format!(
            "{} ({} busy, {} idle, {} offline)",
            data.runners.runners.len(),
            data.runners.busy(),
            data.runners.idle(),
            data.runners.offline(),
        ),
    );

    for runner in &data.runners.runners {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{s}{}{s:#} {d}({}){d:#}",
            runner.name(),
            runner.state.label(),
            s = bold(),
            d = dim()
        );
        let mut row = |key: &str, value: String| {
            let _ = writeln!(out, "{d}  {:<14}{d:#}{value}", key, d = dim());
        };
        row("path", runner.root.display().to_string());
        if let Some(scope) = runner.scope() {
            row("scope", scope.to_string());
        }
        if let Some(version) = &runner.version {
            row("version", version.clone());
        }
        if let Some(service) = &runner.service {
            row("service", service.clone());
        }
        if let Some(id) = runner.config.as_ref().and_then(|c| c.agent_id) {
            row("agent id", id.to_string());
        }
        if !runner.repos.is_empty() {
            row("repos", runner.repos.join(", "));
        }
        if let Some(job) = &runner.current_job {
            row(
                "current job",
                format!(
                    "{} ({} elapsed)",
                    job.full_name(),
                    fmt::duration(job.duration_secs(data.sampled_at))
                ),
            );
            if let Some(repository) = &job.repository {
                row("building", repository.clone());
            }
            if let Some(workspace) = &job.workspace {
                row("workspace", format!("_work/{workspace}"));
            }
        }
        if let Some(path) = &runner.log_path {
            row("log", path.display().to_string());
        }
    }
    out
}

/// Pretty-printed JSON for scripts and pipes.
pub fn json(data: &Dashboard) -> String {
    serde_json::to_string_pretty(data).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}
