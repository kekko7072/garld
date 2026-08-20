//! Native surfaces: the dashboard window and the always-on-top desktop widget.

mod parts;
mod runner_card;
mod widget;
mod window;

use std::path::PathBuf;
use std::time::Instant;

use eframe::egui;

use crate::dashboard::{Dashboard, Source};
use crate::probe::Probe;
use parts::History;

/// Samples kept for the sparklines: at the 1s default that's two minutes.
const HISTORY: usize = 120;

/// Sampling state shared by both surfaces.
///
/// Holds the [`Source`] so CPU deltas stay meaningful across frames, and the
/// rolling series the sparklines draw from.
pub struct Live {
    source: Source,
    pub data: Dashboard,
    pub cpu: History,
    pub mem: History,
    /// Combined CPU of every running job, so the widget can show job load
    /// separately from host load.
    pub job_cpu: History,
    /// Seconds between samples.
    pub interval: f32,
    pub paused: bool,
    last_sample: Instant,
}

impl Live {
    /// Takes the first sample, including the priming wait.
    pub fn new(roots: Vec<PathBuf>, interval: f32) -> Self {
        let mut source = Source::new(roots);
        let data = source.sample();

        let mut live = Self {
            source,
            data,
            cpu: History::new(HISTORY),
            mem: History::new(HISTORY),
            job_cpu: History::new(HISTORY),
            interval,
            paused: false,
            last_sample: Instant::now(),
        };
        live.record();
        live
    }

    /// Resamples if the interval has elapsed. Returns whether it did.
    pub fn tick(&mut self) -> bool {
        if self.paused {
            return false;
        }
        let due = self.interval.max(Probe::min_interval().as_secs_f32());
        if self.last_sample.elapsed().as_secs_f32() < due {
            return false;
        }
        self.data = self.source.sample_now();
        self.last_sample = Instant::now();
        self.record();
        true
    }

    /// Forces a sample on the next tick, whatever the interval.
    pub fn refresh_now(&mut self) {
        self.data = self.source.sample_now();
        self.last_sample = Instant::now();
        self.record();
    }

    fn record(&mut self) {
        self.cpu.push(self.data.snapshot.metrics.cpu_percent);
        self.mem.push(self.data.snapshot.metrics.mem_percent());
        let job_cpu: f32 = self
            .data
            .runners
            .runners
            .iter()
            .map(|runner| runner.job_cpu)
            .sum();
        self.job_cpu.push(job_cpu);
    }

    /// How long until the next sample is due, for `request_repaint_after`.
    pub fn until_next(&self) -> std::time::Duration {
        if self.paused {
            return std::time::Duration::from_millis(250);
        }
        let due = std::time::Duration::from_secs_f32(self.interval.max(0.2));
        due.checked_sub(self.last_sample.elapsed())
            .unwrap_or(std::time::Duration::from_millis(16))
    }
}

/// Opens the full dashboard window.
pub fn run_window(roots: Vec<PathBuf>) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("garld — GitHub Action Runner Local Dashboard")
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([760.0, 460.0])
            .with_app_id("garld"),
        ..Default::default()
    };

    eframe::run_native(
        "garld",
        options,
        Box::new(move |cc| Ok(Box::new(window::WindowApp::new(cc, roots)))),
    )
}

/// Opens the compact desktop widget: borderless, translucent, above other
/// windows, and absent from the taskbar.
pub fn run_widget(roots: Vec<PathBuf>) -> eframe::Result {
    let viewport = egui::ViewportBuilder::default()
        .with_title("garld")
        .with_inner_size([330.0, 232.0])
        .with_min_inner_size([300.0, 200.0])
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_taskbar(false)
        .with_resizable(true)
        .with_app_id("garld-widget");

    let mut options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    // On macOS an "accessory" app has no Dock tile and never steals focus,
    // which is what separates a desktop widget from a very small window.
    #[cfg(target_os = "macos")]
    {
        options.event_loop_builder = Some(Box::new(|builder| {
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            builder.with_activation_policy(ActivationPolicy::Accessory);
        }));
    }

    eframe::run_native(
        "garld widget",
        options,
        Box::new(move |cc| Ok(Box::new(widget::WidgetApp::new(cc, roots)))),
    )
}
