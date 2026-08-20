//! The desktop widget: the runner card, floating above everything else.
//!
//! Same card the dashboard window shows, in a borderless translucent panel that
//! stays on top. Because it has no title bar, the background is a drag handle
//! and it provides its own close button.

use std::path::PathBuf;

use eframe::egui::{self, RichText};

use crate::format as fmt;
use crate::{Surface, spawn_self};

use super::Live;
use super::parts::{self, Palette};
use super::runner_card::{self, CardOptions};

pub struct WidgetApp {
    live: Live,
    card: CardOptions,
}

impl WidgetApp {
    pub fn new(cc: &eframe::CreationContext<'_>, roots: Vec<PathBuf>) -> Self {
        cc.egui_ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(6.0, 5.0);
        });
        Self {
            live: Live::new(roots, 1.5),
            card: CardOptions { history: 6 },
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
            .inner_margin(egui::Margin::same(10));

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

        // Must be "the pointer is over this window", not `has_pointer`, which
        // is true whenever a pointer exists at all. This window floats above
        // everything, so with the wrong predicate the close button sat visible
        // under wherever the cursor happened to be and the next click anywhere
        // on screen dismissed the widget.
        let hovered = ui.rect_contains_pointer(ui.max_rect());

        let palette = Palette::of(ui);
        let data = &self.live.data;
        let now = data.sampled_at;
        let metrics = data.snapshot.metrics.clone();
        let runners = data.runners.runners.clone();

        // A borderless window needs its own way out, but the buttons shouldn't
        // compete with the card for attention when the pointer is elsewhere.
        ui.horizontal(|ui| {
            ui.label(parts::muted(ui, "garld"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if hovered {
                    if ui.small_button("×").on_hover_text("Close widget").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui
                        .small_button("open")
                        .on_hover_text("Open the full dashboard")
                        .clicked()
                    {
                        spawn_self(Surface::Window);
                    }
                } else {
                    // Reserve the same height so the card doesn't shift when
                    // the buttons appear.
                    ui.allocate_space(egui::vec2(0.0, 18.0));
                }
            });
        });

        // Shrink to the cards' own height so the footer sits directly under
        // them instead of being pushed to the bottom of the window; it still
        // grows and scrolls when several runners don't fit.
        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if runners.is_empty() {
                    runner_card::empty_state(ui);
                    return;
                }
                for (index, runner) in runners.iter().enumerate() {
                    if index > 0 {
                        ui.add_space(8.0);
                    }
                    runner_card::show(ui, runner, now, &self.card);
                }
            });

        ui.add_space(6.0);

        // Slim host footer: the widget floats above other work, so the machine's
        // own load is worth a line even when no job is running.
        let cpu_fraction = metrics.cpu_percent / 100.0;
        let mem_fraction = metrics.mem_percent() / 100.0;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("cpu {:.0}%", metrics.cpu_percent))
                    .size(11.0)
                    .monospace()
                    .color(palette.load(cpu_fraction)),
            );
            ui.label(parts::muted(ui, "·"));
            ui.label(
                RichText::new(format!("mem {:.0}%", metrics.mem_percent()))
                    .size(11.0)
                    .monospace()
                    .color(palette.load(mem_fraction)),
            )
            .on_hover_text(format!(
                "{} of {} used",
                fmt::bytes(metrics.mem_used),
                fmt::bytes(metrics.mem_total)
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                parts::sparkline(
                    ui,
                    &self.live.cpu,
                    100.0,
                    egui::vec2(ui.available_width().min(120.0), 14.0),
                    palette.load(cpu_fraction),
                );
            });
        });
    }
}
