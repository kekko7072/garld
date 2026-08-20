//! The menu-bar / notification-area item.
//!
//! Runs its own winit event loop with no window at all. On macOS the status
//! item carries live text next to the icon, which is the closest thing to a
//! native menu-bar readout; on Windows and Linux the same string becomes the
//! tooltip, since those trays show icons only.
//!
//! The icon itself is redrawn as CPU load changes, so the bar shows activity
//! without the menu being open.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

use crate::dashboard::{Dashboard, Source};
use crate::format as fmt;
use crate::runner::RunnerState;
#[cfg(feature = "gui")]
use crate::{Surface, spawn_self};

/// Seconds between samples. Slower than the GUI: a menu bar doesn't need 1 Hz,
/// and this runs all day.
const INTERVAL: Duration = Duration::from_secs(3);

/// Icon edge in pixels. 22 is the macOS menu-bar convention; Windows and Linux
/// scale it down without artefacts.
const ICON_SIZE: u32 = 22;

pub fn run(roots: Vec<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = EventLoop::builder();

    // Without this a menu-bar app also claims a Dock tile, which is wrong for
    // something that only ever lives in the status bar.
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory);
        builder.with_default_menu(false);
    }

    let event_loop = builder.build()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = TrayApp::new(roots);
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// The menu rows whose text is rewritten on every sample.
struct Readout {
    status: MenuItem,
    job: MenuItem,
    host: MenuItem,
    top: MenuItem,
}

struct Actions {
    /// Absent in a tray-only build, which has no window to open.
    #[cfg(feature = "gui")]
    dashboard: MenuId,
    #[cfg(feature = "gui")]
    widget: MenuId,
    quit: MenuId,
}

struct TrayApp {
    source: Source,
    data: Dashboard,
    /// `None` until the platform is ready for us to create it.
    tray: Option<TrayIcon>,
    readout: Option<Readout>,
    actions: Option<Actions>,
    last_sample: Instant,
    /// CPU load in 5% steps, so the icon is only redrawn when it visibly changes.
    last_icon_key: Option<(RunnerState, u8)>,
}

impl TrayApp {
    fn new(roots: Vec<PathBuf>) -> Self {
        let mut source = Source::new(roots);
        let data = source.sample();
        Self {
            source,
            data,
            tray: None,
            readout: None,
            actions: None,
            last_sample: Instant::now(),
            last_icon_key: None,
        }
    }

    /// Builds the status item. Must happen after the event loop starts:
    /// on macOS an `NSStatusItem` needs a running `NSApplication`.
    fn install(&mut self) {
        let menu = Menu::new();

        let status = MenuItem::new("sampling…", false, None);
        let job = MenuItem::new("", false, None);
        let host = MenuItem::new("", false, None);
        let top = MenuItem::new("", false, None);

        let readout_items: [&dyn tray_icon::menu::IsMenuItem; 5] =
            [&status, &job, &host, &top, &PredefinedMenuItem::separator()];
        if menu.append_items(&readout_items).is_err() {
            return;
        }

        // A build without the `gui` feature has no window or widget to open, so
        // it doesn't offer to.
        #[cfg(feature = "gui")]
        let (dashboard, widget) = {
            let dashboard = MenuItem::new("Open Dashboard", true, None);
            let widget = MenuItem::new("Open Desktop Widget", true, None);
            let items: [&dyn tray_icon::menu::IsMenuItem; 3] =
                [&dashboard, &widget, &PredefinedMenuItem::separator()];
            if menu.append_items(&items).is_err() {
                return;
            }
            (dashboard, widget)
        };

        let quit = MenuItem::new("Quit garld", true, None);
        if menu.append(&quit).is_err() {
            return;
        }

        self.actions = Some(Actions {
            #[cfg(feature = "gui")]
            dashboard: dashboard.id().clone(),
            #[cfg(feature = "gui")]
            widget: widget.id().clone(),
            quit: quit.id().clone(),
        });

        let built = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("garld — GitHub Action Runner Local Dashboard")
            .with_menu_on_left_click(true)
            .build();

        match built {
            Ok(tray) => {
                self.tray = Some(tray);
                self.readout = Some(Readout {
                    status,
                    job,
                    host,
                    top,
                });
                self.refresh_display();
            }
            Err(error) => {
                eprintln!("garld: could not create the tray icon: {error}");
            }
        }
    }

    /// Pushes the current sample into the icon, the title and the menu text.
    fn refresh_display(&mut self) {
        let metrics = &self.data.snapshot.metrics;
        let state = self.data.status();
        let headline = self.data.headline();

        if let Some(tray) = &self.tray {
            // macOS renders this beside the icon; elsewhere it's a no-op and
            // the tooltip does the work.
            tray.set_title(Some(&headline));
            let _ = tray.set_tooltip(Some(format!(
                "garld · {headline}\n{} runner(s) · {} processes",
                self.data.runners.runners.len(),
                metrics.process_count,
            )));

            let key = (state, (metrics.cpu_percent / 5.0).round() as u8);
            if self.last_icon_key != Some(key)
                && let Some(icon) = build_icon(state, metrics.cpu_percent / 100.0)
            {
                let _ = tray.set_icon(Some(icon));
                self.last_icon_key = Some(key);
            }
        }

        let Some(readout) = &self.readout else {
            return;
        };

        let report = &self.data.runners;
        readout.status.set_text(if report.runners.is_empty() {
            "No runners on this host".to_string()
        } else {
            format!(
                "{} runner(s) · {} busy · {} idle · {} offline",
                report.runners.len(),
                report.busy(),
                report.idle(),
                report.offline(),
            )
        });

        readout.job.set_text(match self.data.primary_job() {
            Some(job) => {
                let elapsed = report
                    .active_jobs()
                    .into_iter()
                    .map(|(_, j)| j.duration_secs(self.data.sampled_at))
                    .max()
                    .unwrap_or(0);
                format!("▶ {job} · {}", fmt::duration(elapsed))
            }
            None => "No job running".to_string(),
        });

        readout.host.set_text(format!(
            "cpu {:.0}% · mem {} / {}",
            metrics.cpu_percent,
            fmt::bytes(metrics.mem_used),
            fmt::bytes(metrics.mem_total),
        ));

        let busiest = self
            .data
            .snapshot
            .processes
            .iter()
            .max_by(|a, b| {
                a.cpu_percent
                    .partial_cmp(&b.cpu_percent)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|proc| format!("top {} · {:.0}%", proc.name, proc.cpu_percent))
            .unwrap_or_default();
        readout.top.set_text(busiest);
    }

    /// Drains menu clicks. Returns true when the user chose Quit.
    fn handle_menu_events(&mut self) -> bool {
        let Some(actions) = &self.actions else {
            return false;
        };
        let mut quit = false;
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == actions.quit {
                quit = true;
                continue;
            }
            #[cfg(feature = "gui")]
            if event.id == actions.dashboard {
                spawn_self(Surface::Window);
            } else if event.id == actions.widget {
                spawn_self(Surface::Widget);
            }
        }
        quit
    }
}

impl ApplicationHandler for TrayApp {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if matches!(cause, StartCause::Init) {
            self.install();
        }

        if self.handle_menu_events() {
            event_loop.exit();
            return;
        }

        if self.last_sample.elapsed() >= INTERVAL {
            self.data = self.source.sample_now();
            self.last_sample = Instant::now();
            self.refresh_display();
        }
    }

    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        _event: WindowEvent,
    ) {
        // No windows: everything happens in the status item.
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Sleep until the next sample instead of spinning, so an all-day
        // menu-bar process stays at zero CPU between ticks.
        let next = self.last_sample + INTERVAL;
        event_loop.set_control_flow(ControlFlow::WaitUntil(next));
    }
}

/// Draws the status icon: a rounded square outlined in the state colour, filled
/// from the bottom in proportion to host CPU load.
fn build_icon(state: RunnerState, cpu_fraction: f32) -> Option<Icon> {
    let size = ICON_SIZE as usize;
    let mut rgba = vec![0u8; size * size * 4];

    let (r, g, b) = match state {
        RunnerState::Busy => (78, 196, 118),
        RunnerState::Idle => (110, 165, 235),
        RunnerState::Offline => (226, 100, 100),
    };

    // Inset so the glyph doesn't touch the menu bar's edges.
    let inset = 3usize;
    let inner = size - inset * 2;
    let fill_rows = (cpu_fraction.clamp(0.0, 1.0) * inner as f32).round() as usize;
    let fill_top = inset + inner.saturating_sub(fill_rows);

    for y in inset..size - inset {
        for x in inset..size - inset {
            // Clip the four corners to fake a 2px corner radius.
            let dx = (x as i32 - inset as i32).min((size - inset - 1 - x) as i32);
            let dy = (y as i32 - inset as i32).min((size - inset - 1 - y) as i32);
            if dx == 0 && dy == 0 {
                continue;
            }

            let edge = dx == 0 || dy == 0;
            let filled = y >= fill_top;
            let alpha = match (edge, filled) {
                (true, _) => 235,
                (false, true) => 205,
                (false, false) => 45,
            };

            let index = (y * size + x) * 4;
            rgba[index] = r;
            rgba[index + 1] = g;
            rgba[index + 2] = b;
            rgba[index + 3] = alpha;
        }
    }

    Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE).ok()
}
