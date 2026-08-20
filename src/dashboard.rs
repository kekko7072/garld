//! The model every surface renders: one host sample plus the runners on it.

use std::path::PathBuf;

use serde::Serialize;

use crate::probe::{Probe, Snapshot};
use crate::runner::{self, LogCache, RunnerReport};

/// A complete reading of the host at one instant.
#[derive(Debug, Clone, Serialize)]
pub struct Dashboard {
    pub snapshot: Snapshot,
    pub runners: RunnerReport,
    /// Unix epoch seconds the sample was taken.
    pub sampled_at: i64,
}

/// Summaries that only the status item needs: it has one line of text and one
/// icon to say everything in.
#[cfg(feature = "tray")]
impl Dashboard {
    /// The one-line summary used by the tray title and the widget header. Kept
    /// short enough for a macOS menu bar.
    pub fn headline(&self) -> String {
        let busy = self.runners.busy();
        let total = self.runners.runners.len();
        if total == 0 {
            return format!("no runners · {:.0}% cpu", self.snapshot.metrics.cpu_percent);
        }
        if busy > 0 {
            format!(
                "{busy}/{total} busy · {:.0}% cpu",
                self.snapshot.metrics.cpu_percent
            )
        } else {
            format!(
                "{total} idle · {:.0}% cpu",
                self.snapshot.metrics.cpu_percent
            )
        }
    }

    /// Longest-running active job, for surfaces with room for exactly one.
    ///
    /// Labelled with the repository, since `release / ios` alone doesn't say
    /// much on a runner that serves half a dozen repositories.
    pub fn primary_job(&self) -> Option<String> {
        self.runners
            .active_jobs()
            .into_iter()
            .min_by_key(|(_, job)| job.started)
            .map(|(runner, job)| job.labelled(runner.scope()))
    }
}

/// Used by both the widget and the status item to colour a single indicator.
#[cfg(any(feature = "gui", feature = "tray"))]
impl Dashboard {
    /// Overall state, worst-first.
    pub fn status(&self) -> crate::runner::RunnerState {
        use crate::runner::RunnerState;
        if self.runners.busy() > 0 {
            RunnerState::Busy
        } else if self.runners.idle() > 0 {
            RunnerState::Idle
        } else {
            RunnerState::Offline
        }
    }
}

/// Owns the OS handles and caches needed to sample repeatedly.
///
/// CPU figures are deltas between two samples, so a `Source` must outlive
/// individual readings — construct it once per process, not once per tick.
pub struct Source {
    probe: Probe,
    roots: Vec<PathBuf>,
    cache: LogCache,
}

impl Source {
    /// `roots` are extra runner directories to inspect beyond the ones garld
    /// finds by itself.
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            probe: Probe::new(),
            roots,
            cache: LogCache::default(),
        }
    }

    /// Samples, first waiting long enough for CPU counters to be meaningful.
    /// Use for one-shot commands.
    pub fn sample(&mut self) -> Dashboard {
        let snapshot = self.probe.sample();
        self.finish(snapshot)
    }

    /// Samples immediately. Only correct if at least
    /// [`Probe::min_interval`](crate::probe::Probe::min_interval) has elapsed
    /// since the previous sample — true for any live view's tick rate.
    pub fn sample_now(&mut self) -> Dashboard {
        let snapshot = self.probe.sample_now();
        self.finish(snapshot)
    }

    fn finish(&mut self, snapshot: Snapshot) -> Dashboard {
        let runners = runner::discover(&snapshot, &self.roots, &mut self.cache);
        Dashboard {
            snapshot,
            runners,
            sampled_at: runner::now_epoch(),
        }
    }
}
