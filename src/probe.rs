//! Sampling core: turns `sysinfo`'s platform-specific state into flat, owned,
//! serializable snapshots that the CLI, the live view and the GUI all share.

use std::time::{Duration, Instant};

use serde::Serialize;
use sysinfo::{
    CpuRefreshKind, MINIMUM_CPU_UPDATE_INTERVAL, MemoryRefreshKind, ProcessRefreshKind,
    ProcessesToUpdate, RefreshKind, System, UpdateKind, Users,
};

/// Facts about the machine that don't change while we're running.
#[derive(Debug, Clone, Serialize)]
pub struct Host {
    pub hostname: String,
    pub os: String,
    pub os_version: String,
    pub kernel: String,
    pub arch: String,
    pub logical_cores: usize,
    pub physical_cores: Option<usize>,
}

/// Machine-wide numbers, resampled on every tick.
#[derive(Debug, Clone, Serialize)]
pub struct Metrics {
    /// Machine-wide CPU load, 0-100 regardless of core count.
    pub cpu_percent: f32,
    /// Per-core load, each 0-100.
    pub per_core: Vec<f32>,
    pub mem_total: u64,
    pub mem_used: u64,
    pub mem_available: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    /// 1/5/15-minute load average. All zeroes on Windows, which has no equivalent.
    pub load_avg: [f64; 3],
    pub uptime_secs: u64,
    pub process_count: usize,
    /// Total threads, when the platform reports per-process thread lists.
    pub thread_count: Option<usize>,
}

impl Metrics {
    pub fn mem_percent(&self) -> f32 {
        percent_of(self.mem_used, self.mem_total)
    }

    pub fn swap_percent(&self) -> f32 {
        percent_of(self.swap_used, self.swap_total)
    }

    /// True when the platform actually reports load averages.
    pub fn has_load_avg(&self) -> bool {
        self.load_avg.iter().any(|v| *v > 0.0)
    }
}

/// One process, as of one sample.
#[derive(Debug, Clone, Serialize)]
pub struct ProcInfo {
    pub pid: u32,
    pub parent: Option<u32>,
    pub name: String,
    pub user: Option<String>,
    /// Share of a *single* core, so a busy multithreaded process can exceed 100.
    pub cpu_percent: f32,
    pub mem_bytes: u64,
    pub mem_percent: f32,
    pub virtual_bytes: u64,
    pub status: String,
    /// Wall-clock seconds since the process started.
    pub run_secs: u64,
    /// CPU time consumed, in CPU-milliseconds.
    pub cpu_time_ms: u64,
    pub threads: Option<usize>,
    pub exe: Option<String>,
    pub command: String,
    pub cwd: Option<String>,
    /// Bytes read/written since the previous sample.
    pub disk_read: u64,
    pub disk_written: u64,
    pub disk_read_total: u64,
    pub disk_written_total: u64,
    /// Unix epoch seconds.
    pub start_time: u64,
}

impl ProcInfo {
    /// Combined I/O since the previous sample, used for `--sort io`.
    pub fn io_rate(&self) -> u64 {
        self.disk_read.saturating_add(self.disk_written)
    }

    fn matches(&self, needle: &str) -> bool {
        let needle = needle.to_lowercase();
        self.name.to_lowercase().contains(&needle)
            || self.command.to_lowercase().contains(&needle)
            || self
                .exe
                .as_deref()
                .is_some_and(|e| e.to_lowercase().contains(&needle))
            || self.pid.to_string() == needle
    }
}

/// A complete sample: the machine, its metrics, and every visible process.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub host: Host,
    pub metrics: Metrics,
    pub processes: Vec<ProcInfo>,
}

/// Which column to order processes by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    /// CPU share, busiest first.
    Cpu,
    /// Resident memory, largest first.
    Mem,
    /// Process id, lowest first.
    Pid,
    /// Process name, A-Z.
    Name,
    /// Accumulated CPU time, largest first.
    Time,
    /// Disk I/O since the last sample, busiest first.
    Io,
    /// Owning user, A-Z.
    User,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Cpu => "cpu",
            SortKey::Mem => "mem",
            SortKey::Pid => "pid",
            SortKey::Name => "name",
            SortKey::Time => "time",
            SortKey::Io => "io",
            SortKey::User => "user",
        }
    }
}

impl std::fmt::Display for SortKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// How to narrow and order a snapshot's process list for display.
#[derive(Debug, Clone, Default)]
pub struct Query {
    /// Case-insensitive substring match on name, command line, exe path or exact pid.
    pub filter: Option<String>,
    /// Exact (case-insensitive) owning-user match.
    pub user: Option<String>,
    pub sort: SortKeyOrDefault,
    /// Flip the sort direction.
    pub reverse: bool,
    /// Keep at most this many rows. `None` keeps all.
    pub limit: Option<usize>,
}

/// `Query::sort` defaults to CPU without making `SortKey` itself have a default.
#[derive(Debug, Clone, Copy)]
pub struct SortKeyOrDefault(pub SortKey);

impl Default for SortKeyOrDefault {
    fn default() -> Self {
        Self(SortKey::Cpu)
    }
}

/// Applies a query to a process list, returning the rows to display.
///
/// Rows are cloned rather than borrowed so callers can keep results across
/// samples; process lists are small enough (thousands, not millions) that this
/// costs less than threading lifetimes through every renderer.
pub fn select(processes: &[ProcInfo], query: &Query) -> Vec<ProcInfo> {
    let mut rows: Vec<ProcInfo> = processes
        .iter()
        .filter(|p| {
            query
                .filter
                .as_deref()
                .is_none_or(|needle| needle.is_empty() || p.matches(needle))
        })
        .filter(|p| {
            query.user.as_deref().is_none_or(|want| {
                p.user
                    .as_deref()
                    .is_some_and(|have| have.eq_ignore_ascii_case(want))
            })
        })
        .cloned()
        .collect();

    // Descending for "how much", ascending for "which one" — the direction a
    // reader expects from each column. `--reverse` flips whatever that is.
    let descending = match query.sort.0 {
        SortKey::Cpu | SortKey::Mem | SortKey::Time | SortKey::Io => true,
        SortKey::Pid | SortKey::Name | SortKey::User => false,
    };

    rows.sort_by(|a, b| {
        let ord = match query.sort.0 {
            SortKey::Cpu => a
                .cpu_percent
                .partial_cmp(&b.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal),
            SortKey::Mem => a.mem_bytes.cmp(&b.mem_bytes),
            SortKey::Pid => a.pid.cmp(&b.pid),
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Time => a.cpu_time_ms.cmp(&b.cpu_time_ms),
            SortKey::Io => a.io_rate().cmp(&b.io_rate()),
            SortKey::User => a
                .user
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .cmp(&b.user.as_deref().unwrap_or("").to_lowercase()),
        };
        let ord = if descending { ord.reverse() } else { ord };
        // Tie-break on pid so equal rows don't shuffle between samples.
        ord.then_with(|| a.pid.cmp(&b.pid))
    });

    if query.reverse {
        rows.reverse();
    }
    if let Some(limit) = query.limit {
        rows.truncate(limit);
    }
    rows
}

/// Holds the OS handles between samples.
///
/// CPU percentages are computed from the delta between two refreshes, so a
/// `Probe` must be kept alive across ticks — a fresh one every sample would
/// report zero for everything.
pub struct Probe {
    sys: System,
    users: Users,
    last_refresh: Instant,
}

impl Probe {
    /// Takes the priming sample. Cheap; no sleeping here.
    pub fn new() -> Self {
        let refresh = RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
            .with_memory(MemoryRefreshKind::everything())
            .with_processes(process_refresh());

        let mut sys = System::new_with_specifics(refresh);
        sys.refresh_specifics(refresh);

        Self {
            sys,
            users: Users::new_with_refreshed_list(),
            last_refresh: Instant::now(),
        }
    }

    /// Samples the machine, waiting first if the previous refresh was too
    /// recent for the kernel's CPU counters to have moved.
    ///
    /// Without this wait every CPU figure reads 0.0 — the classic one-shot
    /// process-viewer bug.
    pub fn sample(&mut self) -> Snapshot {
        let since = self.last_refresh.elapsed();
        if since < MINIMUM_CPU_UPDATE_INTERVAL {
            std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL - since);
        }
        self.sample_now()
    }

    /// Samples immediately. Only correct when at least
    /// [`MINIMUM_CPU_UPDATE_INTERVAL`] has passed since the last sample.
    pub fn sample_now(&mut self) -> Snapshot {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.sys
            .refresh_processes_specifics(ProcessesToUpdate::All, true, process_refresh());
        self.last_refresh = Instant::now();

        let mem_total = self.sys.total_memory();
        let mut processes: Vec<ProcInfo> = Vec::with_capacity(self.sys.processes().len());
        let mut thread_total = 0usize;
        let mut saw_threads = false;

        for (pid, proc) in self.sys.processes() {
            let threads = proc.tasks().map(|t| t.len());
            if let Some(n) = threads {
                saw_threads = true;
                thread_total += n;
            }

            let mem_bytes = proc.memory();
            let command = proc
                .cmd()
                .iter()
                .map(|a| a.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            let disk = proc.disk_usage();

            // A process the current user may not open reports no start time —
            // routine on Windows, where the runner's service processes belong
            // to `LocalSystem`. `run_time` then measures from the epoch and
            // reports decades, so treat a missing start as an unknown age.
            let start_time = proc.start_time();
            let run_secs = if start_time == 0 { 0 } else { proc.run_time() };

            processes.push(ProcInfo {
                pid: pid.as_u32(),
                parent: proc.parent().map(|p| p.as_u32()),
                name: proc.name().to_string_lossy().into_owned(),
                user: proc
                    .user_id()
                    .and_then(|uid| self.users.get_user_by_id(uid))
                    .map(|u| u.name().to_owned()),
                cpu_percent: proc.cpu_usage(),
                mem_bytes,
                mem_percent: percent_of(mem_bytes, mem_total),
                virtual_bytes: proc.virtual_memory(),
                status: proc.status().to_string(),
                run_secs,
                cpu_time_ms: proc.accumulated_cpu_time(),
                threads,
                exe: proc
                    .exe()
                    .map(|p| p.to_string_lossy().into_owned())
                    .filter(|s| !s.is_empty()),
                // Kernel threads and sandboxed processes expose no argv; the
                // name in brackets is what `ps` shows there too.
                command: if command.is_empty() {
                    format!("[{}]", proc.name().to_string_lossy())
                } else {
                    command
                },
                cwd: proc.cwd().map(|p| p.to_string_lossy().into_owned()),
                disk_read: disk.read_bytes,
                disk_written: disk.written_bytes,
                disk_read_total: disk.total_read_bytes,
                disk_written_total: disk.total_written_bytes,
                start_time,
            });
        }

        let per_core: Vec<f32> = self.sys.cpus().iter().map(|c| c.cpu_usage()).collect();
        let load = System::load_average();

        Snapshot {
            host: Host {
                hostname: System::host_name().unwrap_or_else(|| "unknown".into()),
                os: System::name().unwrap_or_else(|| "unknown".into()),
                os_version: System::os_version().unwrap_or_else(|| "unknown".into()),
                kernel: System::kernel_version().unwrap_or_else(|| "unknown".into()),
                arch: System::cpu_arch(),
                logical_cores: per_core.len(),
                physical_cores: System::physical_core_count(),
            },
            metrics: Metrics {
                cpu_percent: self.sys.global_cpu_usage(),
                per_core,
                mem_total,
                mem_used: self.sys.used_memory(),
                mem_available: self.sys.available_memory(),
                swap_total: self.sys.total_swap(),
                swap_used: self.sys.used_swap(),
                load_avg: [load.one, load.five, load.fifteen],
                uptime_secs: System::uptime(),
                process_count: processes.len(),
                thread_count: saw_threads.then_some(thread_total),
            },
            processes,
        }
    }

    /// How long to wait before `sample_now` produces meaningful CPU numbers.
    pub fn min_interval() -> Duration {
        MINIMUM_CPU_UPDATE_INTERVAL
    }
}

impl Default for Probe {
    fn default() -> Self {
        Self::new()
    }
}

/// The per-process fields we actually display. Requesting only these keeps
/// sampling cheap — `everything()` would also read every process's environment.
fn process_refresh() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory()
        .with_disk_usage()
        .with_user(UpdateKind::OnlyIfNotSet)
        .with_exe(UpdateKind::OnlyIfNotSet)
        .with_cmd(UpdateKind::OnlyIfNotSet)
        .with_cwd(UpdateKind::OnlyIfNotSet)
        .with_tasks()
}

fn percent_of(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64 * 100.0) as f32
    }
}
