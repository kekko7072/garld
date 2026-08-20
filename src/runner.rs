//! Discovery and local state of self-hosted GitHub Actions runners.
//!
//! Everything here comes from the host itself — the runner's own install
//! directory, its `_diag` logs, and the live process table. Nothing is fetched
//! from GitHub, so garld needs no token and works offline.
//!
//! Deliberately never read: `.credentials`, `.credentials_rsaparams`, `.env`
//! and `.path`. The first two hold the runner's private key and auth token; the
//! others routinely carry secrets. Only `.runner` and `.service` are opened.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::probe::{ProcInfo, Snapshot};

/// Files inside a runner root that garld must never open.
///
/// Enforced by `tests::forbidden_files_are_not_read`, which greps this module
/// for reads of these names. It exists only to keep that guarantee honest as
/// the file grows, so it's unreferenced outside tests.
#[cfg_attr(not(test), allow(dead_code))]
const FORBIDDEN: [&str; 4] = [".credentials", ".credentials_rsaparams", ".env", ".path"];

/// How much of the tail of a `Runner_*.log` to scan for job events.
const LOG_TAIL_BYTES: u64 = 512 * 1024;

/// How many finished jobs to keep per runner.
const HISTORY_LIMIT: usize = 25;

/// How much of a `Worker_*.log` to scan for the job's checkout directory. The
/// path can appear well past the first pages when reusable workflows are used.
const WORKER_HEAD_BYTES: u64 = 256 * 1024;

/// The subset of `.runner` we display. Server URLs are ignored — they're
/// internal broker endpoints, not useful on a dashboard.
#[derive(Debug, Clone, Serialize)]
pub struct RunnerConfig {
    pub agent_id: Option<u64>,
    pub agent_name: Option<String>,
    pub pool_name: Option<String>,
    /// e.g. `https://github.com/my-org`.
    pub github_url: Option<String>,
    pub work_folder: Option<String>,
}

impl RunnerConfig {
    /// The org or user the runner is registered to, taken from `github_url`.
    pub fn scope(&self) -> Option<&str> {
        self.github_url
            .as_deref()?
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
    }

    fn read(root: &Path) -> Option<Self> {
        // `.runner` is UTF-8 with a BOM, which serde_json rejects.
        let raw = std::fs::read_to_string(root.join(".runner")).ok()?;
        let raw = raw.trim_start_matches('\u{feff}');
        let value: serde_json::Value = serde_json::from_str(raw).ok()?;

        Some(Self {
            agent_id: value.get("agentId").and_then(|v| v.as_u64()),
            agent_name: string_field(&value, "agentName"),
            pool_name: string_field(&value, "poolName"),
            github_url: string_field(&value, "gitHubUrl"),
            work_folder: string_field(&value, "workFolder"),
        })
    }
}

fn string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .filter(|s| !s.is_empty())
}

/// How a job ended, as the runner reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobResult {
    Succeeded,
    Failed,
    Canceled,
    /// The server gave up on the job — usually the runner died mid-job.
    Abandoned,
    /// A result string we don't recognise.
    Other,
}

impl JobResult {
    fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "canceled" | "cancelled" => Self::Canceled,
            "abandoned" => Self::Abandoned,
            _ => Self::Other,
        }
    }

    /// Only the window's tooltip spells the result out; the CLI uses [`glyph`].
    ///
    /// [`glyph`]: JobResult::glyph
    #[cfg(feature = "gui")]
    pub fn label(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::Abandoned => "abandoned",
            Self::Other => "unknown",
        }
    }

    /// A one-character marker for dense terminal and tray output.
    pub fn glyph(self) -> char {
        match self {
            Self::Succeeded => '✓',
            Self::Failed => '✗',
            Self::Canceled => '⊘',
            Self::Abandoned => '!',
            Self::Other => '?',
        }
    }
}

/// One job, as reconstructed from the listener log.
#[derive(Debug, Clone, Serialize)]
pub struct Job {
    /// Workflow name, the part before `/` in `Running job: release / ios`.
    pub workflow: String,
    /// Job name, the part after `/`.
    pub job: String,
    /// Unix epoch seconds when the runner picked the job up.
    pub started: i64,
    /// Unix epoch seconds when it finished; `None` while still running.
    pub finished: Option<i64>,
    pub result: Option<JobResult>,
    /// Checkout directory under `_work`, when we can tell which one it is.
    pub workspace: Option<String>,
    /// `owner/repo` the job is building, resolved from `_PipelineMapping`.
    pub repository: Option<String>,
}

impl Job {
    /// Seconds elapsed: to completion if finished, else up to `now`.
    pub fn duration_secs(&self, now: i64) -> u64 {
        let end = self.finished.unwrap_or(now);
        (end - self.started).max(0) as u64
    }

    /// `release / ios`
    pub fn full_name(&self) -> String {
        format!("{} / {}", self.workflow, self.job)
    }

    /// The repository without its owner, when the owner is the runner's own
    /// scope. A runner registered to one org shows `management`; a repo from
    /// somewhere else keeps its full `owner/repo` so the difference is visible.
    pub fn short_repo(&self, scope: Option<&str>) -> Option<&str> {
        let repository = self.repository.as_deref()?;
        match (repository.split_once('/'), scope) {
            (Some((owner, name)), Some(scope)) if owner.eq_ignore_ascii_case(scope) => Some(name),
            _ => Some(repository),
        }
    }

    /// `management · release / ios`, or just the job when the repo is unknown.
    pub fn labelled(&self, scope: Option<&str>) -> String {
        match self.short_repo(scope) {
            Some(repository) => format!("{repository} · {}", self.full_name()),
            None => self.full_name(),
        }
    }
}

/// Whether a runner is up, and if so whether it's working.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunnerState {
    /// Listener running, no job.
    Idle,
    /// Listener running with at least one `Runner.Worker`.
    Busy,
    /// Installed, but no listener process — service stopped or never started.
    Offline,
}

impl RunnerState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Busy => "busy",
            Self::Offline => "offline",
        }
    }
}

/// A runner install on this host, plus whatever it's doing right now.
#[derive(Debug, Clone, Serialize)]
pub struct Runner {
    pub root: PathBuf,
    pub config: Option<RunnerConfig>,
    /// Runner release, scraped from the listener log's `Version:` line.
    pub version: Option<String>,
    /// Service identifier from `.service`, e.g. `actions.runner.org.host`.
    pub service: Option<String>,
    pub state: RunnerState,

    /// `runsvc.sh`/`run.sh` wrapper, when the runner runs as a service.
    pub supervisor_pid: Option<u32>,
    pub listener_pid: Option<u32>,
    pub worker_pids: Vec<u32>,

    /// Listener's own usage — should stay near idle.
    pub listener_cpu: f32,
    pub listener_mem: u64,
    /// Seconds the listener has been up.
    pub listener_uptime: u64,

    /// Worker processes *and every descendant*, i.e. the running job's real cost.
    pub job_cpu: f32,
    pub job_mem: u64,
    /// Every pid in the running job's tree, workers included.
    pub job_pids: Vec<u32>,

    pub current_job: Option<Job>,
    /// Finished jobs, newest first.
    pub recent_jobs: Vec<Job>,
    /// Repositories this runner has built, from `_work/_PipelineMapping`.
    pub repos: Vec<String>,
    pub log_path: Option<PathBuf>,
}

impl Runner {
    /// Best available display name: configured agent name, else the directory.
    pub fn name(&self) -> String {
        self.config
            .as_ref()
            .and_then(|c| c.agent_name.clone())
            .unwrap_or_else(|| {
                self.root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| self.root.display().to_string())
            })
    }

    /// The org or user this runner serves.
    pub fn scope(&self) -> Option<&str> {
        self.config.as_ref().and_then(|c| c.scope())
    }

    /// Processes in the running job's tree.
    pub fn job_proc_count(&self) -> usize {
        self.job_pids.len()
    }

    /// Success rate over retained history, as a fraction. `None` with no history.
    pub fn success_rate(&self) -> Option<f32> {
        let judged: Vec<&Job> = self
            .recent_jobs
            .iter()
            .filter(|j| j.result.is_some_and(|r| r != JobResult::Other))
            .collect();
        if judged.is_empty() {
            return None;
        }
        let ok = judged
            .iter()
            .filter(|j| j.result == Some(JobResult::Succeeded))
            .count();
        Some(ok as f32 / judged.len() as f32)
    }
}

/// What a process is, to a runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PidRole {
    /// `runsvc.sh`/`run.sh` service wrapper.
    Supervisor,
    /// `Runner.Listener` — polls GitHub for work.
    Listener,
    /// `Runner.Worker` — executes one job.
    Worker,
    /// Spawned by a worker: a step's compiler, test runner, shell.
    JobStep,
}

impl PidRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::Supervisor => "svc",
            Self::Listener => "listener",
            Self::Worker => "worker",
            Self::JobStep => "job",
        }
    }
}

/// Everything garld found, in one value.
#[derive(Debug, Clone, Serialize)]
pub struct RunnerReport {
    pub runners: Vec<Runner>,
    /// Roots that were checked and rejected, for `--explain`-style output.
    pub searched: Vec<PathBuf>,
}

impl RunnerReport {
    pub fn busy(&self) -> usize {
        self.count(RunnerState::Busy)
    }

    pub fn idle(&self) -> usize {
        self.count(RunnerState::Idle)
    }

    pub fn offline(&self) -> usize {
        self.count(RunnerState::Offline)
    }

    fn count(&self, state: RunnerState) -> usize {
        self.runners.iter().filter(|r| r.state == state).count()
    }

    /// Maps every runner-owned pid to its role and its runner's index.
    ///
    /// More specific roles win: a worker is also in its own subtree, and must
    /// be labelled `Worker` rather than `JobStep`.
    pub fn pid_roles(&self) -> HashMap<u32, (PidRole, usize)> {
        let mut roles = HashMap::new();
        for (index, runner) in self.runners.iter().enumerate() {
            for pid in &runner.job_pids {
                roles.insert(*pid, (PidRole::JobStep, index));
            }
            for pid in &runner.worker_pids {
                roles.insert(*pid, (PidRole::Worker, index));
            }
            if let Some(pid) = runner.listener_pid {
                roles.insert(pid, (PidRole::Listener, index));
            }
            if let Some(pid) = runner.supervisor_pid {
                roles.insert(pid, (PidRole::Supervisor, index));
            }
        }
        roles
    }

    /// Jobs executing across all runners.
    #[cfg(feature = "tray")]
    pub fn active_jobs(&self) -> Vec<(&Runner, &Job)> {
        self.runners
            .iter()
            .filter_map(|r| r.current_job.as_ref().map(|j| (r, j)))
            .collect()
    }
}

/// Memoizes parsed log tails between samples.
///
/// A listener log is appended to, never rewritten, so an unchanged byte length
/// means the job history is unchanged too. Live views resample once a second;
/// without this they would re-read and re-parse half a megabyte every tick.
#[derive(Default)]
pub struct LogCache {
    entries: HashMap<PathBuf, ParsedLog>,
    /// Worker log -> what its job was building. A finished job's log is
    /// immutable, so this is cached by path with no revalidation. The live
    /// job's log is the only one that can still grow, and it is re-read until
    /// it yields a repository.
    origins: HashMap<PathBuf, JobOrigin>,
}

#[derive(Default, Clone)]
struct ParsedLog {
    len: u64,
    version: Option<String>,
    open: Option<Job>,
    finished: Vec<Job>,
}

impl LogCache {
    fn load(&mut self, path: Option<&Path>) -> ParsedLog {
        let Some(path) = path else {
            return ParsedLog::default();
        };
        let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

        if let Some(hit) = self.entries.get(path)
            && hit.len == len
        {
            return hit.clone();
        }

        let log = read_tail(path);
        let (open, finished) = parse_jobs(&log);
        let parsed = ParsedLog {
            len,
            version: parse_version(&log),
            open,
            finished,
        };
        self.entries.insert(path.to_path_buf(), parsed.clone());
        parsed
    }

    fn origin(&mut self, path: &Path) -> JobOrigin {
        // A running job's log may not have reached the mapping path yet, so a
        // miss is retried on the next sample rather than cached as final.
        if let Some(hit) = self.origins.get(path)
            && hit.repository.is_some()
        {
            return hit.clone();
        }
        let resolved = origin_from_worker_log(path);
        self.origins.insert(path.to_path_buf(), resolved.clone());
        resolved
    }
}

/// Finds every runner on the host and reads its current state.
///
/// `extra_roots` are directories the user named explicitly; they're checked
/// even if they don't look like runner installs, so a typo is visible rather
/// than silently ignored.
pub fn discover(
    snapshot: &Snapshot,
    extra_roots: &[PathBuf],
    cache: &mut LogCache,
) -> RunnerReport {
    let children = child_map(&snapshot.processes);
    let by_pid: HashMap<u32, &ProcInfo> = snapshot.processes.iter().map(|p| (p.pid, p)).collect();

    // Roots proven to exist by a live process outrank guesses from the filesystem.
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut searched: Vec<PathBuf> = Vec::new();

    for proc in &snapshot.processes {
        if let Some(root) = root_from_process(proc) {
            push_unique(&mut roots, root);
        }
    }
    for root in extra_roots {
        push_unique(&mut roots, root.clone());
    }
    for candidate in candidate_roots() {
        searched.push(candidate.clone());
        if candidate.join(".runner").is_file() {
            push_unique(&mut roots, candidate);
        }
    }

    let mut runners: Vec<Runner> = Vec::with_capacity(roots.len());
    for root in roots {
        runners.push(inspect(root, snapshot, &children, &by_pid, cache));
    }

    // Busy first, then idle, then offline; alphabetical inside each group.
    runners.sort_by(|a, b| {
        let rank = |s: RunnerState| match s {
            RunnerState::Busy => 0,
            RunnerState::Idle => 1,
            RunnerState::Offline => 2,
        };
        rank(a.state)
            .cmp(&rank(b.state))
            .then_with(|| a.name().to_lowercase().cmp(&b.name().to_lowercase()))
    });

    RunnerReport { runners, searched }
}

/// Reads one runner root's config, log and process state.
fn inspect(
    root: PathBuf,
    snapshot: &Snapshot,
    children: &HashMap<u32, Vec<u32>>,
    by_pid: &HashMap<u32, &ProcInfo>,
    cache: &mut LogCache,
) -> Runner {
    let config = RunnerConfig::read(&root);
    let work = root.join(
        config
            .as_ref()
            .and_then(|c| c.work_folder.clone())
            .unwrap_or_else(|| "_work".to_string()),
    );

    let mut listener_pid = None;
    let mut supervisor_pid = None;
    let mut worker_pids = Vec::new();

    for proc in &snapshot.processes {
        if !belongs_to(proc, &root) {
            continue;
        }
        match classify(proc) {
            Some(Role::Listener) => listener_pid = Some(proc.pid),
            Some(Role::Worker) => worker_pids.push(proc.pid),
            Some(Role::Supervisor) => supervisor_pid = Some(proc.pid),
            None => {}
        }
    }
    worker_pids.sort_unstable();

    let (listener_cpu, listener_mem, listener_uptime) = listener_pid
        .and_then(|pid| by_pid.get(&pid))
        .map(|p| (p.cpu_percent, p.mem_bytes, p.run_secs))
        .unwrap_or((0.0, 0, 0));

    // A job's true cost is the worker plus everything it spawned — compilers,
    // test runners, docker clients. Walk the subtree rather than trusting the
    // worker's own counters, which stay near zero while children do the work.
    let mut job_cpu = 0.0;
    let mut job_mem = 0;
    let mut job_pids: Vec<u32> = Vec::new();
    for pid in &worker_pids {
        for descendant in subtree(*pid, children) {
            if job_pids.contains(&descendant) {
                continue;
            }
            if let Some(proc) = by_pid.get(&descendant) {
                job_cpu += proc.cpu_percent;
                job_mem += proc.mem_bytes;
                job_pids.push(descendant);
            }
        }
    }
    job_pids.sort_unstable();

    let log_path = newest_log(&root.join("_diag"), "Runner_");
    let parsed = cache.load(log_path.as_deref());
    let version = parsed.version;
    let mut current_job = parsed.open;
    let recent_jobs = parsed.finished;

    // The log alone can't say whether a job is still going: if the runner is
    // killed mid-job no completion line is ever written. A live worker process
    // is the authority, so an unfinished trailing job without one is history.
    let mut recent_jobs = recent_jobs;
    if worker_pids.is_empty()
        && let Some(job) = current_job.take()
    {
        recent_jobs.insert(0, job);
        recent_jobs.truncate(HISTORY_LIMIT);
    }

    // Which repository each job was building. The listener log never says, so
    // this comes from the checkout directory each job used, joined against the
    // runner's own pipeline mapping.
    let pipeline = PipelineMap::read(&work);
    let diag = root.join("_diag");
    let worker_logs = worker_log_index(&diag);

    if let Some(job) = current_job.as_mut() {
        let origin = match_worker_log(&worker_logs, job.started)
            .map(|path| cache.origin(path))
            .unwrap_or_default();
        // A job seconds old hasn't written its mapping path yet; the most
        // recently touched checkout directory covers that gap.
        job.workspace = origin.workspace.or_else(|| active_workspace(&work));
        job.repository = origin.repository.or_else(|| {
            job.workspace
                .as_deref()
                .and_then(|directory| pipeline.repository(directory))
        });
    }

    for job in &mut recent_jobs {
        if let Some(path) = match_worker_log(&worker_logs, job.started) {
            let origin = cache.origin(path);
            job.workspace = origin.workspace;
            job.repository = origin.repository.or_else(|| {
                job.workspace
                    .as_deref()
                    .and_then(|directory| pipeline.repository(directory))
            });
        }
    }

    let state = if !worker_pids.is_empty() {
        RunnerState::Busy
    } else if listener_pid.is_some() {
        RunnerState::Idle
    } else {
        RunnerState::Offline
    };

    Runner {
        config,
        version,
        service: read_service(&root),
        state,
        supervisor_pid,
        listener_pid,
        worker_pids,
        listener_cpu,
        listener_mem,
        listener_uptime,
        job_cpu,
        job_mem,
        job_pids,
        current_job,
        recent_jobs,
        repos: pipeline.repositories(),
        log_path,
        root,
    }
}

/// An executable's file name, minus a Windows `.exe` suffix.
///
/// Deliberately not `Path::file_stem`: the runner's binaries are named
/// `Runner.Listener` and `Runner.Worker`, and `file_stem` reads `.Listener` as
/// an extension and returns `Runner` — which matches nothing.
fn binary_name(path: &Path) -> Option<String> {
    // Split on both separators rather than using `Path::file_name`, whose
    // notion of a separator is the *host's*. A Windows-shaped path read from a
    // process table should parse the same way wherever garld is built, and
    // Windows accepts `/` too, so handling both is right on every platform.
    let text = path.to_string_lossy();
    let last = text.rsplit(['/', '\\']).next()?;
    if last.is_empty() {
        return None;
    }
    Some(strip_exe(last))
}

fn strip_exe(name: &str) -> String {
    match name.len().checked_sub(4) {
        Some(cut) if name[cut..].eq_ignore_ascii_case(".exe") => name[..cut].to_string(),
        _ => name.to_string(),
    }
}

/// What part a process plays in a runner install.
enum Role {
    Supervisor,
    Listener,
    Worker,
}

fn classify(proc: &ProcInfo) -> Option<Role> {
    let stem = proc
        .exe
        .as_deref()
        .and_then(|e| binary_name(Path::new(e)))
        .unwrap_or_else(|| strip_exe(&proc.name));

    match stem.as_str() {
        "Runner.Listener" => Some(Role::Listener),
        "Runner.Worker" => Some(Role::Worker),
        _ => {
            // Wrapper scripts show up as bash/sh with the script in argv.
            let cmd = proc.command.as_str();
            if cmd.contains("runsvc.sh") || cmd.contains("run-helper.sh") || cmd.contains("run.sh")
            {
                Some(Role::Supervisor)
            } else {
                None
            }
        }
    }
}

/// Whether a process belongs to the install at `root`, by path prefix.
fn belongs_to(proc: &ProcInfo, root: &Path) -> bool {
    let root = root.to_string_lossy();
    proc.exe.as_deref().is_some_and(|e| e.starts_with(&*root)) || proc.command.contains(&*root)
}

/// Recovers an install root from a runner process's own paths.
fn root_from_process(proc: &ProcInfo) -> Option<PathBuf> {
    if let Some(exe) = proc.exe.as_deref() {
        let path = Path::new(exe);
        let stem = binary_name(path)?;
        if stem == "Runner.Listener" || stem == "Runner.Worker" {
            // <root>/bin/Runner.Listener
            let root = path.parent()?.parent()?;
            if root.join(".runner").is_file() {
                return Some(root.to_path_buf());
            }
        }
    }

    // Service wrapper: the script's absolute path sits in argv.
    for script in ["runsvc.sh", "run-helper.sh", "run.sh"] {
        if let Some(idx) = proc.command.find(script) {
            let prefix = &proc.command[..idx];
            let root = Path::new(prefix.trim_end_matches('/').trim_end_matches('\\'));
            if root.join(".runner").is_file() {
                return Some(root.to_path_buf());
            }
        }
    }
    None
}

/// Directories worth checking for an install, so stopped runners still appear.
///
/// Honours `GARLD_RUNNER_DIRS` (platform path separator) first.
fn candidate_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();

    if let Some(list) = std::env::var_os("GARLD_RUNNER_DIRS") {
        out.extend(std::env::split_paths(&list));
    }

    let mut bases: Vec<PathBuf> = Vec::new();
    if let Some(home) = home_dir() {
        bases.push(home.clone());
        bases.push(home.join("Developer"));
    }
    if cfg!(windows) {
        bases.push(PathBuf::from("C:\\"));
        bases.push(PathBuf::from("C:\\Users\\Public"));
    } else {
        bases.push(PathBuf::from("/opt"));
        bases.push(PathBuf::from("/usr/local"));
        bases.push(PathBuf::from("/var/lib"));
    }

    // Match by prefix instead of globbing — several runners on one host are
    // conventionally actions-runner, actions-runner-2, actions-runner-arm64…
    const PREFIXES: [&str; 3] = ["actions-runner", "github-runner", "runner"];
    for base in bases {
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if PREFIXES.iter().any(|p| name.starts_with(p)) {
                out.push(entry.path());
            }
        }
    }
    out
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// The service identifier from `.service`, which holds a unit or plist path.
fn read_service(root: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(root.join(".service")).ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    Path::new(raw)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .or_else(|| Some(raw.to_owned()))
}

/// Maps a checkout directory under `_work` to the `owner/repo` that owns it.
///
/// The runner records this itself in
/// `_work/_PipelineMapping/<owner>/<repo>/PipelineFolder.json`, which names both
/// the repository and the pipeline directory it checks out into. That join is
/// what lets garld report the repository a job is building — the listener log
/// never mentions it.
#[derive(Default)]
struct PipelineMap {
    /// pipeline directory -> `owner/repo`
    by_directory: HashMap<String, String>,
}

impl PipelineMap {
    fn read(work: &Path) -> Self {
        let mut by_directory = HashMap::new();
        let Ok(owners) = std::fs::read_dir(work.join("_PipelineMapping")) else {
            return Self { by_directory };
        };

        for owner in owners.flatten() {
            let Ok(entries) = std::fs::read_dir(owner.path()) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(raw) = std::fs::read_to_string(entry.path().join("PipelineFolder.json"))
                else {
                    continue;
                };
                // Same BOM as `.runner`.
                let Ok(value) =
                    serde_json::from_str::<serde_json::Value>(raw.trim_start_matches('\u{feff}'))
                else {
                    continue;
                };

                let repository = string_field(&value, "repositoryName").unwrap_or_else(|| {
                    format!(
                        "{}/{}",
                        owner.file_name().to_string_lossy(),
                        entry.file_name().to_string_lossy()
                    )
                });
                // Fall back to the directory name, which is what the runner
                // uses when it isn't recorded explicitly.
                let directory = string_field(&value, "pipelineDirectory")
                    .unwrap_or_else(|| entry.file_name().to_string_lossy().into_owned());

                by_directory.insert(directory, repository);
            }
        }
        Self { by_directory }
    }

    fn repository(&self, directory: &str) -> Option<String> {
        self.by_directory.get(directory).cloned()
    }

    /// Every repository this runner has built, alphabetically.
    fn repositories(&self) -> Vec<String> {
        let mut repos: Vec<String> = self.by_directory.values().cloned().collect();
        repos.sort();
        repos.dedup();
        repos
    }
}

/// A job's own `Worker_*.log`, keyed by the start time in its filename.
///
/// Worker logs are named `Worker_YYYYMMDD-HHMMSS-utc.log` and the worker spawns
/// a second or two after the listener logs `Running job:`, which is what lets a
/// job be matched to its log.
fn worker_log_index(diag: &Path) -> Vec<(i64, PathBuf)> {
    let mut index = Vec::new();
    let Ok(entries) = std::fs::read_dir(diag) else {
        return index;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(stamp) = name.strip_prefix("Worker_")
            && let Some(started) = parse_log_stamp(stamp)
        {
            index.push((started, entry.path()));
        }
    }
    index.sort_by_key(|(started, _)| *started);
    index
}

/// Parses the `20260820-194758` in a worker log's filename to epoch seconds.
fn parse_log_stamp(stamp: &str) -> Option<i64> {
    let date = stamp.get(..8)?;
    let time = stamp.get(9..15)?;
    if stamp.as_bytes().get(8) != Some(&b'-') {
        return None;
    }
    let year: i64 = date.get(..4)?.parse().ok()?;
    let month: u32 = date.get(4..6)?.parse().ok()?;
    let day: u32 = date.get(6..8)?.parse().ok()?;
    let hour: i64 = time.get(..2)?.parse().ok()?;
    let minute: i64 = time.get(2..4)?.parse().ok()?;
    let second: i64 = time.get(4..6)?.parse().ok()?;
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// The worker log belonging to a job that started at `started`.
///
/// The window is deliberately tight: a wrong match would label a job with
/// another repository, which is worse than leaving it unlabelled.
fn match_worker_log(index: &[(i64, PathBuf)], started: i64) -> Option<&Path> {
    index
        .iter()
        .filter(|(stamp, _)| (started - 10..=started + 90).contains(stamp))
        .min_by_key(|(stamp, _)| (stamp - started).abs())
        .map(|(_, path)| path.as_path())
}

/// What a job was building, recovered from its own `Worker_*.log`.
#[derive(Default, Clone)]
struct JobOrigin {
    /// `owner/repo`.
    repository: Option<String>,
    /// Checkout directory under `_work`.
    workspace: Option<String>,
}

/// Reads a worker log to find the repository its job was building.
///
/// The decisive marker is the job's own pipeline-mapping path,
/// `/_work/_PipelineMapping/<owner>/<repo>`, which names the repository
/// outright. Exactly one appears per job — including jobs that call reusable
/// workflows from another repository, which is why this is preferred over
/// scanning for repository names directly: the *workflow's* repo is mentioned
/// first and much earlier, so a naive first-match would report the wrong one.
///
/// The checkout path `/_work/<dir>/<dir>` is read too when present, but it
/// lands hundreds of kilobytes into the log, so it can't be relied on alone.
fn origin_from_worker_log(path: &Path) -> JobOrigin {
    let text = read_head(path, WORKER_HEAD_BYTES);
    let stop = |c: char| c == '/' || c == '\\' || c == '\'' || c == '"' || c.is_whitespace();

    let mut repository = None;
    if let Some(index) = text.find("_PipelineMapping/") {
        let rest = &text[index + "_PipelineMapping/".len()..];
        let owner: String = rest.chars().take_while(|c| !stop(*c)).collect();
        let after = &rest[owner.len()..];
        if !owner.is_empty() && after.starts_with('/') {
            let name: String = after[1..].chars().take_while(|c| !stop(*c)).collect();
            if !name.is_empty() {
                repository = Some(format!("{owner}/{name}"));
            }
        }
    }

    let mut workspace = None;
    for (index, _) in text.match_indices("/_work/") {
        let rest = &text[index + "/_work/".len()..];
        let directory: String = rest.chars().take_while(|c| !stop(*c)).collect();
        // Skip the runner's own scratch trees, and require the path to descend
        // into the directory so a bare `_work/<dir>` reference doesn't count.
        if directory.is_empty()
            || directory.starts_with('_')
            || !rest[directory.len()..].starts_with('/')
        {
            continue;
        }
        workspace = Some(directory);
        break;
    }

    JobOrigin {
        repository,
        workspace,
    }
}

/// Reads at most `limit` bytes from the start of a file.
fn read_head(path: &Path, limit: u64) -> String {
    let Ok(file) = File::open(path) else {
        return String::new();
    };
    let mut buf = Vec::new();
    if std::io::Read::take(file, limit)
        .read_to_end(&mut buf)
        .is_err()
    {
        return String::new();
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// The checkout directory most recently touched, which is the running job's
/// workspace. Underscore-prefixed entries are the runner's own scratch dirs.
fn active_workspace(work: &Path) -> Option<String> {
    let entries = std::fs::read_dir(work).ok()?;
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            !name.starts_with('_') && e.path().is_dir()
        })
        .filter_map(|e| {
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((modified, e.file_name().to_string_lossy().into_owned()))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, name)| name)
}

/// The most recently modified log with the given filename prefix.
fn newest_log(diag: &Path, prefix: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(diag).ok()?;
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .filter_map(|e| {
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((modified, e.path()))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

/// Reads the tail of a log file.
///
/// Listener logs grow for the lifetime of the service and reach tens of
/// megabytes, so only the end is read. A partial first line is dropped.
fn read_tail(path: &Path) -> String {
    let Ok(mut file) = File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let from_start = len <= LOG_TAIL_BYTES;
    if !from_start && file.seek(SeekFrom::End(-(LOG_TAIL_BYTES as i64))).is_err() {
        return String::new();
    }

    let mut buf = Vec::with_capacity(LOG_TAIL_BYTES.min(len) as usize);
    if file.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    if from_start {
        text
    } else {
        match text.find('\n') {
            Some(idx) => text[idx + 1..].to_string(),
            None => String::new(),
        }
    }
}

/// Scrapes `[... INFO Listener] Version: 2.336.0`.
fn parse_version(log: &str) -> Option<String> {
    log.lines().find_map(|line| {
        let rest = line.split("Version: ").nth(1)?;
        let version = rest
            .trim()
            .trim_matches('\'')
            .split_whitespace()
            .next()?
            .trim_matches('\'');
        version
            .chars()
            .next()
            .filter(char::is_ascii_digit)
            .map(|_| version.to_string())
    })
}

/// Rebuilds job history from listener log lines.
///
/// The runner writes a matched pair per job:
///   `[<ts> INFO Terminal] WRITE LINE: <ts>: Running job: <workflow> / <job>`
///   `[<ts> INFO Terminal] WRITE LINE: <ts>: Job <workflow> / <job> completed with result: <r>`
///
/// Returns the still-running job, if the log ends mid-job, and finished jobs
/// newest first.
fn parse_jobs(log: &str) -> (Option<Job>, Vec<Job>) {
    let mut open: Option<Job> = None;
    let mut finished: Vec<Job> = Vec::new();

    for line in log.lines() {
        let Some(timestamp) = leading_timestamp(line) else {
            continue;
        };

        if let Some(rest) = line.split("Running job: ").nth(1) {
            // A second start without a completion means we lost the end of the
            // previous job — keep it, unresolved, rather than dropping it.
            if let Some(previous) = open.take() {
                finished.push(previous);
            }
            let (workflow, job) = split_job_name(rest.trim());
            open = Some(Job {
                workflow,
                job,
                started: timestamp,
                finished: None,
                result: None,
                workspace: None,
                repository: None,
            });
        } else if let Some(rest) = line.split(" completed with result: ").nth(1) {
            let result = JobResult::parse(rest);
            match open.take() {
                Some(mut job) => {
                    job.finished = Some(timestamp);
                    job.result = Some(result);
                    finished.push(job);
                }
                None => {
                    // Completion whose start scrolled out of the tail window.
                    let name = line
                        .split("Job ")
                        .nth(1)
                        .and_then(|s| s.split(" completed with result: ").next())
                        .unwrap_or("unknown")
                        .trim();
                    let (workflow, job) = split_job_name(name);
                    finished.push(Job {
                        workflow,
                        job,
                        started: timestamp,
                        finished: Some(timestamp),
                        result: Some(result),
                        workspace: None,
                        repository: None,
                    });
                }
            }
        }
    }

    finished.reverse();
    finished.truncate(HISTORY_LIMIT);
    (open, finished)
}

/// Splits `release / ios` into workflow and job. Job names may contain further
/// slashes (matrix legs), so only the first separator counts.
fn split_job_name(name: &str) -> (String, String) {
    match name.split_once(" / ") {
        Some((workflow, job)) => (workflow.trim().to_string(), job.trim().to_string()),
        None => (name.trim().to_string(), String::new()),
    }
}

/// Parses the epoch seconds out of a `[2026-08-20 19:11:52Z ...]` line prefix.
fn leading_timestamp(line: &str) -> Option<i64> {
    let inner = line.strip_prefix('[')?;
    let date = inner.get(..10)?;
    let time = inner.get(11..19)?;
    if inner.as_bytes().get(10) != Some(&b' ') {
        return None;
    }

    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;

    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;

    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Days between 1970-01-01 and the given date, proleptic Gregorian.
///
/// Hinnant's `days_from_civil`: exact for all dates we'll ever see, and avoids
/// a date-library dependency for what is ultimately one arithmetic expression.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = month as i64;
    let day = day as i64;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Current time in Unix epoch seconds.
pub fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// parent pid -> child pids, for subtree walks.
fn child_map(processes: &[ProcInfo]) -> HashMap<u32, Vec<u32>> {
    let mut map: HashMap<u32, Vec<u32>> = HashMap::new();
    for proc in processes {
        if let Some(parent) = proc.parent {
            map.entry(parent).or_default().push(proc.pid);
        }
    }
    map
}

/// Every pid in the tree rooted at `pid`, including `pid` itself.
///
/// Guards against cycles: pid reuse can briefly make the reported parent table
/// inconsistent, and a naive walk would then loop forever.
fn subtree(pid: u32, children: &HashMap<u32, Vec<u32>>) -> Vec<u32> {
    let mut seen = vec![pid];
    let mut stack = vec![pid];
    while let Some(current) = stack.pop() {
        for child in children.get(&current).into_iter().flatten() {
            if !seen.contains(child) {
                seen.push(*child);
                stack.push(*child);
            }
        }
    }
    seen
}

fn push_unique(roots: &mut Vec<PathBuf>, root: PathBuf) {
    if !roots.contains(&root) {
        roots.push(root);
    }
}

/// Compile-time assurance that the secret-bearing files are never named in a
/// read path. Referenced by the test below so the list can't rot silently.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_files_are_not_read() {
        let source = include_str!("runner.rs");
        for name in FORBIDDEN {
            let reads = source.matches(&format!("join(\"{name}\")")).count();
            assert_eq!(reads, 0, "{name} must never be opened");
        }
    }

    #[test]
    fn parses_listener_timestamps() {
        let ts = leading_timestamp("[2026-08-20 19:11:52Z INFO Terminal] WRITE LINE: x").unwrap();
        assert_eq!(ts, 1_787_253_112);
        assert!(leading_timestamp("no timestamp here").is_none());
    }

    #[test]
    fn epoch_matches_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        assert_eq!(days_from_civil(2026, 8, 20), 20685);
    }

    #[test]
    fn pairs_job_start_and_completion() {
        let log = "\
[2026-08-20 19:04:25Z INFO Terminal] WRITE LINE: 2026-08-20 19:04:25Z: Running job: release / ios
[2026-08-20 19:11:52Z INFO Terminal] WRITE LINE: 2026-08-20 19:11:52Z: Job release / ios completed with result: Succeeded
[2026-08-20 19:12:00Z INFO Terminal] WRITE LINE: 2026-08-20 19:12:00Z: Running job: release / macos
";
        let (open, finished) = parse_jobs(log);
        let open = open.expect("trailing job should still be open");
        assert_eq!(open.workflow, "release");
        assert_eq!(open.job, "macos");
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].result, Some(JobResult::Succeeded));
        assert_eq!(finished[0].duration_secs(0), 447);
    }

    #[test]
    fn keeps_unresolved_job_when_completion_is_missing() {
        let log = "\
[2026-08-20 18:00:00Z INFO Terminal] WRITE LINE: Running job: build / one
[2026-08-20 18:05:00Z INFO Terminal] WRITE LINE: Running job: build / two
";
        let (open, finished) = parse_jobs(log);
        assert_eq!(open.unwrap().job, "two");
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].job, "one");
        assert!(finished[0].result.is_none());
    }

    #[test]
    fn reads_version_line() {
        let log = "[2026-08-20 07:59:49Z INFO Listener] Version: 2.336.0\n";
        assert_eq!(parse_version(log).as_deref(), Some("2.336.0"));
    }

    #[test]
    fn runner_binaries_keep_their_dotted_names() {
        // The whole point: file_stem() would yield "Runner" for both of these.
        assert_eq!(
            binary_name(Path::new("/home/u/actions-runner/bin/Runner.Listener")).unwrap(),
            "Runner.Listener"
        );
        assert_eq!(
            binary_name(Path::new("C:\\actions-runner\\bin\\Runner.Worker.exe")).unwrap(),
            "Runner.Worker"
        );
        assert_eq!(strip_exe("Runner.Listener"), "Runner.Listener");
        assert_eq!(strip_exe("node.EXE"), "node");
        assert_eq!(strip_exe("exe"), "exe");
    }

    #[test]
    fn subtree_survives_a_parent_cycle() {
        let mut children = HashMap::new();
        children.insert(1u32, vec![2u32]);
        children.insert(2u32, vec![1u32, 3u32]);
        let mut found = subtree(1, &children);
        found.sort_unstable();
        assert_eq!(found, vec![1, 2, 3]);
    }
}
