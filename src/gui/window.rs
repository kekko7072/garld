//! The full dashboard window: runners down the left, processes in the middle.

use std::path::PathBuf;

use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};

use crate::format as fmt;
use crate::probe::{ProcInfo, Query, SortKey, SortKeyOrDefault, select};
use crate::runner::{Job, Runner, RunnerState};

use super::Live;
use super::parts::{self, Palette};

pub struct WindowApp {
    live: Live,
    filter: String,
    sort: SortKey,
    reverse: bool,
    runners_only: bool,
    /// 0 means no limit.
    limit: usize,
    selected: Option<u32>,
}

impl WindowApp {
    pub fn new(cc: &eframe::CreationContext<'_>, roots: Vec<PathBuf>) -> Self {
        // Slightly tighter than egui's default, so a dense table stays legible.
        cc.egui_ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        });

        Self {
            live: Live::new(roots, 1.0),
            filter: String::new(),
            sort: SortKey::Cpu,
            reverse: false,
            // Runner processes are the point of the dashboard; the whole
            // host is one click away.
            runners_only: true,
            limit: 0,
            selected: None,
        }
    }

    fn query(&self) -> Query {
        Query {
            filter: (!self.filter.is_empty()).then(|| self.filter.clone()),
            user: None,
            sort: SortKeyOrDefault(self.sort),
            reverse: self.reverse,
            limit: (self.limit > 0).then_some(self.limit),
        }
    }
}

impl eframe::App for WindowApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.live.tick();

        egui::Panel::top("host")
            .exact_size(112.0)
            .show(ui, |ui| self.host_panel(ui));

        egui::Panel::left("runners")
            .default_size(370.0)
            .size_range(280.0..=560.0)
            .resizable(true)
            .show(ui, |ui| self.runners_panel(ui));

        if let Some(pid) = self.selected {
            let found = self
                .live
                .data
                .snapshot
                .processes
                .iter()
                .find(|p| p.pid == pid)
                .cloned();
            match found {
                Some(proc) => {
                    egui::Panel::bottom("detail")
                        .resizable(true)
                        .default_size(160.0)
                        .show(ui, |ui| self.detail_panel(ui, &proc));
                }
                // The process exited between frames; drop the selection rather
                // than leaving an empty panel pinned open.
                None => self.selected = None,
            }
        }

        egui::CentralPanel::default().show(ui, |ui| self.process_panel(ui));

        ui.ctx().request_repaint_after(self.live.until_next());
    }
}

impl WindowApp {
    fn host_panel(&mut self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        let host = self.live.data.snapshot.host.clone();
        let metrics = self.live.data.snapshot.metrics.clone();

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("garld").strong().size(15.0));
            ui.label(parts::muted(ui, format!("· {}", host.hostname)));
            ui.label(parts::muted(
                ui,
                format!(
                    "· {} {} · {} · {} cores · up {}",
                    host.os,
                    host.os_version,
                    host.arch,
                    host.logical_cores,
                    fmt::duration(metrics.uptime_secs),
                ),
            ));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .selectable_label(self.live.paused, "⏸")
                    .on_hover_text("Pause sampling")
                    .clicked()
                {
                    self.live.paused = !self.live.paused;
                }
                if ui.button("⟳").on_hover_text("Sample now").clicked() {
                    self.live.refresh_now();
                }
                ui.add(
                    egui::Slider::new(&mut self.live.interval, 0.25..=10.0)
                        .suffix("s")
                        .logarithmic(true)
                        .text("refresh"),
                );
            });
        });

        ui.add_space(4.0);

        // Three meters side by side, each stacking its meter over its history.
        // The inner layout must be forced top-down: `allocate_ui` inherits the
        // parent's horizontal layout, which would put the sparkline beside the
        // meter and squeeze it to nothing.
        ui.horizontal(|ui| {
            let column = (ui.available_width() - 44.0) / 3.0;
            let stack = egui::Layout::top_down(egui::Align::Min);

            ui.allocate_ui_with_layout(egui::vec2(column, 52.0), stack, |ui| {
                let fraction = metrics.cpu_percent / 100.0;
                parts::metric_row(
                    ui,
                    "cpu",
                    fraction,
                    &format!("{:.0}%", metrics.cpu_percent),
                    palette.load(fraction),
                );
                parts::sparkline(
                    ui,
                    &self.live.cpu,
                    100.0,
                    egui::vec2(ui.available_width(), 22.0),
                    palette.load(fraction),
                );
            });

            ui.allocate_ui_with_layout(egui::vec2(column, 52.0), stack, |ui| {
                let fraction = metrics.mem_percent() / 100.0;
                parts::metric_row(
                    ui,
                    "mem",
                    fraction,
                    &format!(
                        "{} / {}",
                        fmt::bytes(metrics.mem_used),
                        fmt::bytes(metrics.mem_total)
                    ),
                    palette.load(fraction),
                );
                parts::sparkline(
                    ui,
                    &self.live.mem,
                    100.0,
                    egui::vec2(ui.available_width(), 22.0),
                    palette.load(fraction),
                );
            });

            ui.allocate_ui_with_layout(egui::vec2(column, 52.0), stack, |ui| {
                // Job CPU is summed across workers, so it scales past 100% —
                // chart it against the whole machine's capacity.
                let capacity = (host.logical_cores as f32) * 100.0;
                let job_cpu = self.live.job_cpu.latest();
                parts::metric_row(
                    ui,
                    "jobs",
                    job_cpu / capacity,
                    &format!("{job_cpu:.0}%"),
                    if job_cpu > 0.0 { palette.ok } else { palette.muted },
                );
                parts::sparkline(
                    ui,
                    &self.live.job_cpu,
                    self.live.job_cpu.peak(capacity * 0.25),
                    egui::vec2(ui.available_width(), 22.0),
                    palette.ok,
                );
            });
        });
    }

    fn runners_panel(&mut self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        let report = &self.live.data.runners;
        let now = self.live.data.sampled_at;

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Runners").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                parts::pill(
                    ui,
                    &format!("{} offline", report.offline()),
                    if report.offline() > 0 {
                        palette.bad
                    } else {
                        palette.muted
                    },
                );
                parts::pill(ui, &format!("{} idle", report.idle()), palette.idle);
                parts::pill(
                    ui,
                    &format!("{} busy", report.busy()),
                    if report.busy() > 0 {
                        palette.ok
                    } else {
                        palette.muted
                    },
                );
            });
        });
        ui.add_space(6.0);

        if report.runners.is_empty() {
            parts::card(ui, |ui| {
                ui.label(RichText::new("No runner installs found").strong());
                ui.label(parts::muted(
                    ui,
                    "garld looks at running processes and the usual install paths. \
                     Pass --runner-dir, or set GARLD_RUNNER_DIRS.",
                ));
            });
            return;
        }

        let runners = report.runners.clone();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for runner in &runners {
                    self.runner_card(ui, runner, now);
                    ui.add_space(8.0);
                }
            });
    }

    fn runner_card(&mut self, ui: &mut egui::Ui, runner: &Runner, now: i64) {
        let palette = Palette::of(ui);
        parts::card(ui, |ui| {
            ui.horizontal(|ui| {
                parts::pill(ui, runner.state.label(), palette.state(runner.state));
                ui.label(RichText::new(runner.name()).strong());
            });

            if let Some(scope) = runner.scope() {
                let mut line = scope.to_string();
                if let Some(version) = &runner.version {
                    line.push_str(&format!(" · v{version}"));
                }
                if let Some(pool) = runner.config.as_ref().and_then(|c| c.pool_name.as_deref()) {
                    line.push_str(&format!(" · pool {pool}"));
                }
                ui.label(parts::muted(ui, line));
            }

            ui.add_space(4.0);

            match (&runner.current_job, runner.state) {
                (Some(job), _) => self.active_job(ui, runner, job, now),
                (None, RunnerState::Idle) => {
                    ui.label(parts::muted(
                        ui,
                        format!(
                            "waiting for work · listener up {}",
                            fmt::duration(runner.listener_uptime)
                        ),
                    ));
                }
                (None, _) => {
                    ui.label(
                        RichText::new("not running")
                            .size(11.0)
                            .color(palette.bad),
                    );
                    if let Some(service) = &runner.service {
                        ui.label(parts::muted(ui, format!("service {service}")));
                    }
                }
            }

            if !runner.recent_jobs.is_empty() {
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(parts::muted(ui, "recent"));
                    if let Some(rate) = runner.success_rate() {
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.label(parts::muted(ui, format!("{:.0}% pass", rate * 100.0)));
                            },
                        );
                    }
                });
                for job in runner.recent_jobs.iter().take(6) {
                    self.history_row(ui, job, runner.scope());
                }
            }
        });
    }

    fn active_job(&self, ui: &mut egui::Ui, runner: &Runner, job: &Job, now: i64) {
        let palette = Palette::of(ui);

        // The repository leads: it's the part that differs between jobs on a
        // runner that serves many repos.
        if let Some(repository) = job.short_repo(runner.scope()) {
            ui.label(
                RichText::new(repository)
                    .strong()
                    .size(12.0)
                    .color(palette.idle),
            )
            .on_hover_text(job.repository.clone().unwrap_or_default());
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new(job.full_name()).strong().color(palette.ok));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(fmt::duration(job.duration_secs(now)))
                        .size(11.0)
                        .monospace()
                        .color(palette.warn),
                );
            });
        });
        ui.label(parts::muted(
            ui,
            format!(
                "{:.0}% cpu · {} · {} processes",
                runner.job_cpu,
                fmt::bytes(runner.job_mem),
                runner.job_proc_count(),
            ),
        ));
    }

    fn history_row(&self, ui: &mut egui::Ui, job: &Job, scope: Option<&str>) {
        let palette = Palette::of(ui);
        let color = palette.result(job.result);
        ui.horizontal(|ui| {
            parts::dot(ui, color, job.result.is_some());
            ui.label(RichText::new(fmt::truncate(&job.labelled(scope), 32)).size(11.0))
                .on_hover_text(match job.result {
                    Some(result) => format!(
                        "{}\n{} — {}",
                        job.repository.as_deref().unwrap_or("unknown repository"),
                        job.full_name(),
                        result.label()
                    ),
                    None => format!("{} — no result recorded", job.full_name()),
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(parts::muted(
                    ui,
                    fmt::duration(job.duration_secs(job.finished.unwrap_or(job.started))),
                ));
            });
        });
    }

    fn process_panel(&mut self, ui: &mut egui::Ui) {
        let roles = self.live.data.runners.pid_roles();

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("name, command, path or pid")
                    .desired_width(220.0),
            );
            if ui.button("×").on_hover_text("Clear filter").clicked() {
                self.filter.clear();
            }
            ui.checkbox(&mut self.runners_only, "Runner processes only");

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(parts::muted(
                    ui,
                    format!("{} total", self.live.data.snapshot.metrics.process_count),
                ));
            });
        });
        ui.add_space(4.0);

        let pool: Vec<ProcInfo> = if self.runners_only {
            self.live
                .data
                .snapshot
                .processes
                .iter()
                .filter(|p| roles.contains_key(&p.pid))
                .cloned()
                .collect()
        } else {
            self.live.data.snapshot.processes.clone()
        };
        let rows = select(&pool, &self.query());

        let palette = Palette::of(ui);
        let text_height = egui::TextStyle::Body.resolve(ui.style()).size + 6.0;

        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(64.0))
            .column(Column::exact(66.0))
            .column(Column::initial(110.0).at_least(60.0))
            .column(Column::exact(62.0))
            .column(Column::exact(62.0))
            .column(Column::exact(72.0))
            .column(Column::exact(76.0))
            .column(Column::remainder().at_least(120.0))
            .header(22.0, |mut header| {
                let mut column = |header: &mut egui_extras::TableRow<'_, '_>,
                                  title: &str,
                                  key: Option<SortKey>| {
                    header.col(|ui| {
                        let active = key == Some(self.sort);
                        let mut label = title.to_string();
                        if active {
                            // ASCII on purpose: egui's bundled fonts have no
                            // triangle glyphs, so ▴/▾ would render as tofu.
                            label.push_str(if self.reverse { " ^" } else { " v" });
                        }
                        let text = RichText::new(label).size(11.0).strong();
                        match key {
                            Some(key) => {
                                if ui
                                    .add(egui::Button::new(text).frame(false))
                                    .on_hover_text(format!("Sort by {key}"))
                                    .clicked()
                                {
                                    if active {
                                        self.reverse = !self.reverse;
                                    } else {
                                        self.sort = key;
                                        self.reverse = false;
                                    }
                                }
                            }
                            None => {
                                ui.label(text);
                            }
                        }
                    });
                };

                column(&mut header, "PID", Some(SortKey::Pid));
                column(&mut header, "ROLE", None);
                column(&mut header, "USER", Some(SortKey::User));
                column(&mut header, "CPU%", Some(SortKey::Cpu));
                column(&mut header, "MEM%", Some(SortKey::Mem));
                column(&mut header, "RSS", Some(SortKey::Mem));
                column(&mut header, "TIME", Some(SortKey::Time));
                column(&mut header, "NAME", Some(SortKey::Name));
            })
            .body(|body| {
                body.rows(text_height, rows.len(), |mut row| {
                    let proc = &rows[row.index()];
                    let role = roles.get(&proc.pid).map(|(role, _)| *role);
                    row.set_selected(self.selected == Some(proc.pid));

                    let tint = match role {
                        Some(crate::runner::PidRole::Listener) => Some(palette.idle),
                        Some(_) => Some(palette.ok),
                        None => None,
                    };

                    row.col(|ui| {
                        ui.label(RichText::new(proc.pid.to_string()).monospace().size(11.0));
                    });
                    row.col(|ui| {
                        if let (Some(role), Some(tint)) = (role, tint) {
                            ui.label(RichText::new(role.label()).size(10.0).color(tint));
                        }
                    });
                    row.col(|ui| {
                        ui.label(
                            RichText::new(proc.user.clone().unwrap_or_else(|| "-".into()))
                                .size(11.0),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            RichText::new(fmt::percent(proc.cpu_percent))
                                .monospace()
                                .size(11.0)
                                .color(palette.load(proc.cpu_percent / 100.0)),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            RichText::new(fmt::percent(proc.mem_percent))
                                .monospace()
                                .size(11.0),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            RichText::new(fmt::bytes(proc.mem_bytes))
                                .monospace()
                                .size(11.0),
                        );
                    });
                    row.col(|ui| {
                        ui.label(
                            RichText::new(fmt::cpu_time(proc.cpu_time_ms))
                                .monospace()
                                .size(11.0),
                        );
                    });
                    row.col(|ui| {
                        let text = RichText::new(&proc.name).size(11.0);
                        ui.label(match tint {
                            Some(tint) => text.color(tint),
                            None => text,
                        })
                        .on_hover_text(&proc.command);
                    });

                    if row.response().clicked() {
                        self.selected = if self.selected == Some(proc.pid) {
                            None
                        } else {
                            Some(proc.pid)
                        };
                    }
                });
            });
    }

    fn detail_panel(&mut self, ui: &mut egui::Ui, proc: &ProcInfo) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(&proc.name).strong().size(14.0));
            ui.label(parts::muted(ui, format!("pid {}", proc.pid)));
            if let Some(parent) = proc.parent {
                ui.label(parts::muted(ui, format!("· parent {parent}")));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    self.selected = None;
                }
            });
        });
        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut field = |label: &str, value: String| {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            egui::vec2(88.0, 16.0),
                            egui::Label::new(parts::muted(ui, label)).selectable(false),
                        );
                        ui.label(RichText::new(value).size(11.0).monospace());
                    });
                };

                field("status", proc.status.clone());
                field("user", proc.user.clone().unwrap_or_else(|| "-".into()));
                field("cpu", format!("{:.1}%", proc.cpu_percent));
                field(
                    "memory",
                    format!(
                        "{} resident, {} virtual ({:.1}% of host)",
                        fmt::bytes(proc.mem_bytes),
                        fmt::bytes(proc.virtual_bytes),
                        proc.mem_percent,
                    ),
                );
                field("cpu time", fmt::cpu_time(proc.cpu_time_ms));
                field("elapsed", fmt::duration(proc.run_secs));
                if let Some(threads) = proc.threads {
                    field("threads", threads.to_string());
                }
                field(
                    "disk",
                    format!(
                        "{} read, {} written · {} / {} since start",
                        fmt::rate(proc.disk_read),
                        fmt::rate(proc.disk_written),
                        fmt::bytes(proc.disk_read_total),
                        fmt::bytes(proc.disk_written_total),
                    ),
                );
                if let Some(exe) = &proc.exe {
                    field("exe", exe.clone());
                }
                if let Some(cwd) = &proc.cwd {
                    field("cwd", cwd.clone());
                }
                field("command", proc.command.clone());
            });
    }
}
