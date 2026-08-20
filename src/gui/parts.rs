//! Drawing pieces shared by the dashboard window and the desktop widget.

use std::collections::VecDeque;

use eframe::egui::{
    self, Color32, CornerRadius, Pos2, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use crate::runner::{JobResult, RunnerState};

/// A rolling series of samples for a sparkline.
#[derive(Clone)]
pub struct History {
    values: VecDeque<f32>,
    capacity: usize,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self {
            values: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, value: f32) {
        if self.values.len() == self.capacity {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }

    pub fn values(&self) -> impl Iterator<Item = f32> + '_ {
        self.values.iter().copied()
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn latest(&self) -> f32 {
        self.values.back().copied().unwrap_or(0.0)
    }

    /// Largest sample, floored at `floor` so a flat quiet series doesn't get
    /// magnified into looking busy.
    pub fn peak(&self, floor: f32) -> f32 {
        self.values.iter().copied().fold(floor, f32::max)
    }
}

/// Semantic colours resolved against the active egui theme.
#[derive(Clone, Copy)]
pub struct Palette {
    pub ok: Color32,
    pub warn: Color32,
    pub bad: Color32,
    pub idle: Color32,
    pub muted: Color32,
    pub surface: Color32,
    pub outline: Color32,
}

impl Palette {
    pub fn of(ui: &Ui) -> Self {
        let visuals = ui.visuals();
        if visuals.dark_mode {
            Self {
                ok: Color32::from_rgb(80, 200, 120),
                warn: Color32::from_rgb(230, 180, 70),
                bad: Color32::from_rgb(230, 100, 100),
                idle: Color32::from_rgb(110, 165, 235),
                muted: Color32::from_rgb(140, 145, 155),
                surface: Color32::from_rgb(32, 34, 39),
                outline: Color32::from_rgb(58, 61, 68),
            }
        } else {
            Self {
                ok: Color32::from_rgb(30, 145, 80),
                warn: Color32::from_rgb(180, 130, 20),
                bad: Color32::from_rgb(195, 60, 60),
                idle: Color32::from_rgb(45, 105, 190),
                muted: Color32::from_rgb(110, 115, 125),
                surface: Color32::from_rgb(246, 247, 249),
                outline: Color32::from_rgb(214, 217, 222),
            }
        }
    }

    /// Green while there's headroom, amber as it tightens, red at saturation.
    pub fn load(&self, fraction: f32) -> Color32 {
        if fraction >= 0.9 {
            self.bad
        } else if fraction >= 0.65 {
            self.warn
        } else {
            self.ok
        }
    }

    pub fn state(&self, state: RunnerState) -> Color32 {
        match state {
            RunnerState::Busy => self.ok,
            RunnerState::Idle => self.idle,
            RunnerState::Offline => self.bad,
        }
    }

    pub fn result(&self, result: Option<JobResult>) -> Color32 {
        match result {
            Some(JobResult::Succeeded) => self.ok,
            Some(JobResult::Failed) => self.bad,
            Some(JobResult::Canceled) => self.warn,
            Some(JobResult::Abandoned) => self.bad,
            _ => self.muted,
        }
    }
}

/// A filled area chart of `history`, scaled to `max`.
pub fn sparkline(ui: &mut Ui, history: &History, max: f32, size: Vec2, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    if history.is_empty() || history.len() < 2 || !ui.is_rect_visible(rect) {
        return response;
    }

    let painter = ui.painter();
    let max = max.max(1.0);
    let count = history.len();
    let step = rect.width() / (count.saturating_sub(1)).max(1) as f32;

    let points: Vec<Pos2> = history
        .values()
        .enumerate()
        .map(|(index, value)| {
            let fraction = (value / max).clamp(0.0, 1.0);
            Pos2::new(
                rect.left() + index as f32 * step,
                rect.bottom() - fraction * rect.height(),
            )
        })
        .collect();

    // Fill under the curve, then stroke the curve itself. Two triangle strips
    // per sample is cheaper than a real polygon tessellation and looks the same
    // for a monotonic-in-x series.
    let fill = color.gamma_multiply(0.15);
    for pair in points.windows(2) {
        let quad = Rect::from_min_max(
            Pos2::new(pair[0].x, pair[0].y.min(pair[1].y)),
            Pos2::new(pair[1].x, rect.bottom()),
        );
        painter.rect_filled(quad, 0.0, fill);
    }
    painter.line(points, Stroke::new(1.4, color));

    response
}

/// A labelled horizontal meter: `cpu   ████░░░░  38%`.
///
/// Returns the row's response so callers can attach a tooltip carrying the
/// detail that doesn't fit in the value cell.
pub fn metric_row(
    ui: &mut Ui,
    label: &str,
    fraction: f32,
    value: &str,
    color: Color32,
) -> Response {
    let palette = Palette::of(ui);
    ui.horizontal(|ui| {
        ui.add_sized(
            Vec2::new(38.0, 14.0),
            egui::Label::new(RichText::new(label).size(11.0).color(palette.muted)).selectable(false),
        );

        let (rect, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width() - 64.0, 8.0),
            Sense::hover(),
        );
        // Cloned so the painter's borrow doesn't outlive the next `ui` use;
        // `Painter` is a handle, so this is cheap.
        let painter = ui.painter().clone();
        painter.rect_filled(rect, CornerRadius::same(4), palette.outline);
        let filled = Rect::from_min_size(
            rect.min,
            Vec2::new(rect.width() * fraction.clamp(0.0, 1.0), rect.height()),
        );
        painter.rect_filled(filled, CornerRadius::same(4), color);

        ui.add_sized(
            Vec2::new(60.0, 14.0),
            egui::Label::new(RichText::new(value).size(11.0).monospace()).selectable(false),
        );
    })
    .response
}

/// A small filled dot.
///
/// Used instead of glyphs like `✓`/`●` for status markers: egui's bundled fonts
/// don't cover them, so they render as tofu boxes. A painted circle always
/// looks right.
pub fn dot(ui: &mut Ui, color: Color32, filled: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(11.0, 11.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if filled {
            painter.circle_filled(rect.center(), 3.6, color);
        } else {
            painter.circle_stroke(rect.center(), 3.2, Stroke::new(1.4, color));
        }
    }
    response
}

/// A small rounded status chip.
pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = egui::FontId::proportional(11.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let padding = Vec2::new(7.0, 3.0);
    let (rect, response) = ui.allocate_exact_size(galley.size() + padding * 2.0, Sense::hover());

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(7), color.gamma_multiply(0.18));
        painter.rect_stroke(
            rect,
            CornerRadius::same(7),
            Stroke::new(1.0, color.gamma_multiply(0.55)),
            StrokeKind::Inside,
        );
        painter.galley(rect.min + padding, galley, color);
    }
    response
}

/// A card container: surface fill, hairline outline, comfortable padding.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let palette = Palette::of(ui);
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, add)
        .inner
}

/// A dimmed caption.
pub fn muted(ui: &Ui, text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(11.0).color(Palette::of(ui).muted)
}
