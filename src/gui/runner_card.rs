//! The runner card, shared by the dashboard window and the desktop widget.
//!
//! One implementation so the widget is literally the same card the window
//! shows, rather than a second thing that drifts from it.

use eframe::egui::{self, RichText, Ui};

use crate::format as fmt;
use crate::runner::{Job, Runner, RunnerState};

use super::parts::{self, Palette};

/// How much of the card to draw.
pub struct CardOptions {
    /// Finished jobs to list. 0 hides the history section.
    pub history: usize,
}

impl Default for CardOptions {
    fn default() -> Self {
        Self { history: 6 }
    }
}

/// Draws one runner as a self-contained card.
pub fn show(ui: &mut Ui, runner: &Runner, now: i64, opts: &CardOptions) {
    let palette = Palette::of(ui);

    parts::card(ui, |ui| {
        ui.horizontal(|ui| {
            parts::pill(ui, runner.state.label(), palette.state(runner.state));
            ui.label(RichText::new(runner.name()).strong().size(14.0));
        });

        // Organisation, runner version and pool — the identity line.
        let mut meta: Vec<String> = Vec::new();
        if let Some(scope) = runner.scope() {
            meta.push(scope.to_string());
        }
        if let Some(version) = &runner.version {
            meta.push(format!("v{version}"));
        }
        if let Some(pool) = runner.config.as_ref().and_then(|c| c.pool_name.as_deref()) {
            meta.push(format!("pool {pool}"));
        }
        if !meta.is_empty() {
            ui.label(parts::muted(ui, meta.join(" · ")));
        }

        ui.add_space(4.0);

        match (&runner.current_job, runner.state) {
            (Some(job), _) => active_job(ui, runner, job, now),
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
                ui.label(RichText::new("not running").size(11.0).color(palette.bad));
                if let Some(service) = &runner.service {
                    ui.label(parts::muted(ui, format!("service {service}")));
                }
            }
        }

        if opts.history > 0 && !runner.recent_jobs.is_empty() {
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(2.0);

            ui.horizontal(|ui| {
                ui.label(parts::muted(ui, "recent"));
                if let Some(rate) = runner.success_rate() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(parts::muted(ui, format!("{:.0}% pass", rate * 100.0)));
                    });
                }
            });

            for job in runner.recent_jobs.iter().take(opts.history) {
                history_row(ui, job, runner.scope());
            }
        }
    });
}

fn active_job(ui: &mut Ui, runner: &Runner, job: &Job, now: i64) {
    let palette = Palette::of(ui);

    // The repository leads: it's the part that differs between jobs on a runner
    // that serves many repositories.
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

fn history_row(ui: &mut Ui, job: &Job, scope: Option<&str>) {
    let palette = Palette::of(ui);
    let color = palette.result(job.result);

    ui.horizontal(|ui| {
        parts::dot(ui, color, job.result.is_some());
        ui.label(RichText::new(fmt::truncate(&job.labelled(scope), 34)).size(11.0))
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

/// Shown in place of cards when the host has no runner installs.
pub fn empty_state(ui: &mut Ui) {
    parts::card(ui, |ui| {
        ui.label(RichText::new("No runner installs found").strong());
        ui.label(parts::muted(
            ui,
            "garld looks at running processes and the usual install paths. \
             Pass --runner-dir, or set GARLD_RUNNER_DIRS.",
        ));
    });
}
