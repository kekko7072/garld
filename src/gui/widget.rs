//! The desktop widget: a small borderless panel that floats above other
//! windows and answers "are my runners busy?" at a glance.
//!
//! Because it has no title bar, the whole background is a drag handle and it
//! provides its own close button.

use std::path::PathBuf;

use eframe::egui::{self, RichText};

use crate::format as fmt;
use crate::runner::RunnerState;

use super::parts::{self, Palette};
use super::{Live, spawn_self};

pub struct WidgetApp {
    live: Live,
    /// Chrome is revealed on hover so the resting state stays uncluttered.
    hovered: bool,
}

impl WidgetApp {
    pub fn new(cc: &eframe::CreationContext<'_>, roots: Vec<PathBuf>) -> Self {
        cc.egui_ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        });
        Self {
            live: Live::new(roots, 1.5),
            hovered: false,
        }
    }
}

impl eframe::App for WidgetApp {
    /// Transparent, so the rounded panel below is the only thing drawn.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.live.tick();
        self.hovered = ctx.input(|i| i.pointer.has_pointer());

        let dark = ui.visuals().dark_mode;
        // Translucent enough to feel like a desktop widget, opaque enough that
        // text stays readable over a busy background.
        let backdrop = if dark {
            egui::Color32::from_rgba_unmultiplied(20, 22, 26, 246)
        } else {
            egui::Color32::from_rgba_unmultiplied(250, 250, 252, 249)
        };

        let frame = egui::Frame::new()
            .fill(backdrop)
            .corner_radius(egui::CornerRadius::same(12))
            .stroke(egui::Stroke::new(
                1.0,
                if dark {
                    egui::Color32::from_rgb(58, 61, 68)
                } else {
                    egui::Color32::from_rgb(214, 217, 222)
                },
            ))
            .inner_margin(egui::Margin::same(12));

        egui::CentralPanel::default()
            .frame(frame)
            .show(ui, |ui| self.body(ui, &ctx));

        ctx.request_repaint_after(self.live.until_next());
    }
}

impl WidgetApp {
    fn body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // Claimed before the content so every widget drawn afterwards keeps
        // click priority; only bare background drags move the window.
        let background = ui.interact(
            ui.max_rect(),
            ui.id().with("drag-surface"),
            egui::Sense::click_and_drag(),
        );
        if background.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        let palette = Palette::of(ui);
        let data = &self.live.data;
        let status = data.status();
        let metrics = data.snapshot.metrics.clone();

        ui.horizontal(|ui| {
            ui.label(RichText::new("garld").strong().size(12.0));
            parts::pill(
                ui,
                match status {
                    RunnerState::Busy => "building",
                    RunnerState::Idle => "idle",
                    RunnerState::Offline => "offline",
                },
                palette.state(status),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.hovered {
                    if ui
                        .small_button("×")
                        .on_hover_text("Close widget")
                        .clicked()
                    {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui
                        .small_button("open")
                        .on_hover_text("Open the full dashboard")
                        .clicked()
                    {
                        spawn_self("gui");
                    }
                }
            });
        });

        ui.add_space(6.0);

        let runners = data.runners.runners.clone();
        if runners.is_empty() {
            ui.label(parts::muted(ui, "no runners on this host"));
        } else {
            for runner in runners.iter().take(3) {
                ui.horizontal(|ui| {
                    parts::dot(
                        ui,
                        palette.state(runner.state),
                        runner.state == RunnerState::Busy,
                    );
                    ui.label(RichText::new(fmt::truncate(&runner.name(), 20)).size(11.0));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match &runner.current_job {
                            Some(job) => {
                                ui.label(
                                    RichText::new(fmt::duration(
                                        job.duration_secs(data.sampled_at),
                                    ))
                                    .size(11.0)
                                    .monospace()
                                    .color(palette.warn),
                                );
                            }
                            None => {
                                ui.label(parts::muted(ui, runner.state.label()));
                            }
                        }
                    });
                });

                if let Some(job) = &runner.current_job {
                    ui.label(
                        RichText::new(fmt::truncate(&job.labelled(runner.scope()), 36))
                            .size(11.0)
                            .color(palette.ok),
                    )
                    .on_hover_text(job.repository.clone().unwrap_or_default());
                    ui.label(parts::muted(
                        ui,
                        format!(
                            "{:.0}% cpu · {} · {} procs",
                            runner.job_cpu,
                            fmt::bytes(runner.job_mem),
                            runner.job_proc_count()
                        ),
                    ));
                }
            }
            if runners.len() > 3 {
                ui.label(parts::muted(ui, format!("+{} more", runners.len() - 3)));
            }
        }

        ui.add_space(6.0);

        let cpu_fraction = metrics.cpu_percent / 100.0;
        parts::metric_row(
            ui,
            "cpu",
            cpu_fraction,
            &format!("{:.0}%", metrics.cpu_percent),
            palette.load(cpu_fraction),
        );
        let mem_fraction = metrics.mem_percent() / 100.0;
        parts::metric_row(
            ui,
            "mem",
            mem_fraction,
            &format!("{:.0}%", metrics.mem_percent()),
            palette.load(mem_fraction),
        )
        .on_hover_text(format!(
            "{} of {} used",
            fmt::bytes(metrics.mem_used),
            fmt::bytes(metrics.mem_total)
        ));

        parts::sparkline(
            ui,
            &self.live.cpu,
            100.0,
            egui::vec2(ui.available_width(), ui.available_height().clamp(22.0, 64.0)),
            palette.load(cpu_fraction),
        );
    }
}
