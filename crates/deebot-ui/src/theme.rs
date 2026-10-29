//! Colors, cards and small animated widgets shared by every screen.

use std::f32::consts::{PI, TAU};

use eframe::egui::{
    self, vec2, Align, Align2, Color32, CornerRadius, FontId, Frame, Id, Layout, Margin, Response, RichText, Sense,
    Shadow, Shape, Stroke, Ui,
};

pub const BG: Color32 = Color32::from_rgb(13, 16, 22);
pub const CARD: Color32 = Color32::from_rgb(24, 29, 39);
pub const CARD_EDGE: Color32 = Color32::from_rgb(40, 47, 61);
pub const TRACK: Color32 = Color32::from_rgb(38, 45, 58);
pub const ACCENT: Color32 = Color32::from_rgb(45, 190, 170);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(28, 92, 90);
pub const OK: Color32 = Color32::from_rgb(76, 178, 104);
pub const WARN: Color32 = Color32::from_rgb(232, 170, 66);
pub const BAD: Color32 = Color32::from_rgb(222, 92, 82);
pub const TEXT: Color32 = Color32::from_rgb(228, 233, 243);
pub const TEXT_WEAK: Color32 = Color32::from_rgb(132, 142, 160);

/// Install the dark theme, rounded corners and roomier spacing.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(TEXT);
    visuals.panel_fill = BG;
    visuals.window_fill = CARD;
    visuals.extreme_bg_color = Color32::from_rgb(18, 22, 30);
    visuals.faint_bg_color = CARD;
    visuals.selection.bg_fill = ACCENT_DIM;
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.hyperlink_color = ACCENT;

    let round = CornerRadius::same(10);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = CARD;
    widgets.noninteractive.corner_radius = round;
    widgets.inactive.weak_bg_fill = Color32::from_rgb(36, 43, 57);
    widgets.inactive.bg_fill = Color32::from_rgb(36, 43, 57);
    widgets.inactive.corner_radius = round;
    widgets.hovered.weak_bg_fill = Color32::from_rgb(48, 57, 74);
    widgets.hovered.bg_fill = Color32::from_rgb(48, 57, 74);
    widgets.hovered.corner_radius = round;
    widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    widgets.active.weak_bg_fill = ACCENT_DIM;
    widgets.active.bg_fill = ACCENT_DIM;
    widgets.active.corner_radius = round;
    widgets.open.corner_radius = round;
    ctx.set_visuals(visuals);

    ctx.global_style_mut(|style| {
        style.spacing.item_spacing = vec2(10.0, 10.0);
        style.spacing.button_padding = vec2(14.0, 8.0);
        style.spacing.interact_size.y = 30.0;
    });
}

/// A rounded, softly shadowed panel with an optional title.
pub fn card<R>(ui: &mut Ui, title: Option<&str>, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::NONE
        .fill(CARD)
        .stroke(Stroke::new(1.0, CARD_EDGE))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::same(18))
        .shadow(Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(110) })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if let Some(title) = title {
                ui.label(RichText::new(title).size(17.0).strong());
                ui.add_space(4.0);
            }
            add_contents(ui)
        })
        .inner
}

/// A label on the left with a control on the right.
pub fn row(ui: &mut Ui, label: &str, add_control: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(Layout::right_to_left(Align::Center), add_control);
    });
}

/// An on/off switch whose knob slides and whose color fades.
pub fn switch(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(46.0, 26.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(response.id, *on, 0.18);
    let hover = ui.ctx().animate_bool_with_time(response.id.with("hover"), response.hovered(), 0.12);
    let track = TRACK.lerp_to_gamma(ACCENT, t);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(13), track);
    if hover > 0.0 {
        painter.rect_stroke(
            rect,
            CornerRadius::same(13),
            Stroke::new(1.0, ACCENT.gamma_multiply(hover)),
            egui::StrokeKind::Inside,
        );
    }
    let knob_x = egui::lerp((rect.left() + 13.0)..=(rect.right() - 13.0), t);
    painter.circle_filled(egui::pos2(knob_x, rect.center().y), 9.5, Color32::WHITE);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A row of pill-shaped tabs that fills the width. Each tab is as wide as its label needs, and the
/// spare space is shared out evenly, so a long label never pushes the row past the screen edge.
/// Returns the index of the tab that was clicked.
pub fn tab_row(ui: &mut Ui, labels: &[&str], selected: usize) -> Option<usize> {
    let pad = 9.0;
    let spacing = ui.spacing().item_spacing.x;
    let mut size = 14.0;
    let mut widths: Vec<f32> = Vec::new();
    // Shrink the text a little if even the minimum widths would not fit
    for attempt in [14.0, 13.0, 12.0] {
        size = attempt;
        widths = labels
            .iter()
            .map(|label| ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(size), TEXT).size().x + 2.0 * pad)
            .collect();
        if widths.iter().sum::<f32>() + spacing * (labels.len() as f32 - 1.0) <= ui.available_width() {
            break;
        }
    }
    let used = widths.iter().sum::<f32>() + spacing * (labels.len() as f32 - 1.0);
    let extra = ((ui.available_width() - used) / labels.len() as f32).clamp(0.0, 40.0);

    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().button_padding = vec2(pad, 8.0);
        for (index, label) in labels.iter().enumerate() {
            let id = ui.scope_id().with(("tab", *label));
            let t = ui.ctx().animate_bool_with_time(id, index == selected, 0.2);
            let fill = Color32::TRANSPARENT.lerp_to_gamma(ACCENT_DIM, t);
            let color = TEXT_WEAK.lerp_to_gamma(TEXT, t);
            let button = egui::Button::new(RichText::new(*label).size(size).color(color))
                .fill(fill)
                .stroke(Stroke::NONE)
                .corner_radius(CornerRadius::same(20))
                .min_size(vec2(widths[index] + extra, 34.0));
            if ui.add(button).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                clicked = Some(index);
            }
        }
    });
    clicked
}

/// A filled call-to-action button.
pub fn action(ui: &mut Ui, label: &str, fill: Color32) -> Response {
    let button = egui::Button::new(RichText::new(label).size(15.0).strong().color(Color32::WHITE))
        .fill(fill)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(12))
        .min_size(vec2(124.0, 42.0));
    ui.add(button).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Buttons in two equal columns that fill the width. Returns the index of the one clicked.
/// Each entry is (label, fill color, enabled).
pub fn button_grid(ui: &mut Ui, buttons: &[(&str, Color32, bool)]) -> Option<usize> {
    let width = ((ui.available_width() - ui.spacing().item_spacing.x) / 2.0).max(80.0);
    let mut clicked = None;
    for (row, pair) in buttons.chunks(2).enumerate() {
        ui.horizontal(|ui| {
            for (column, (label, fill, enabled)) in pair.iter().enumerate() {
                let button = egui::Button::new(RichText::new(*label).size(15.0).strong().color(Color32::WHITE))
                    .fill(*fill)
                    .stroke(Stroke::NONE)
                    .corner_radius(CornerRadius::same(12))
                    .min_size(vec2(width, 44.0));
                if ui.add_enabled(*enabled, button).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    clicked = Some(row * 2 + column);
                }
            }
        });
    }
    clicked
}

/// A dot that gently pulses while `active`.
pub fn pulse_dot(ui: &mut Ui, color: Color32, active: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
    let painter = ui.painter();
    if active {
        let time = ui.input(|i| i.time) as f32;
        let wave = (time * 3.0).sin() * 0.5 + 0.5;
        painter.circle_filled(rect.center(), 5.0 + 4.0 * wave, color.gamma_multiply(0.35 * (1.0 - wave) + 0.1));
        ui.ctx().request_repaint();
    }
    painter.circle_filled(rect.center(), 5.0, color);
}

fn battery_color(percent: f32) -> Color32 {
    if percent >= 40.0 {
        OK
    } else if percent >= 20.0 {
        WARN
    } else {
        BAD
    }
}

/// A ring that sweeps to the battery level.
pub fn battery_ring(ui: &mut Ui, percent: Option<i64>, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let id = Id::unique("battery-ring");
    let shown = ui.ctx().animate_value_with_time(id, percent.unwrap_or(0) as f32, 0.9);
    let center = rect.center();
    let radius = size * 0.40;
    let thickness = size * 0.085;
    let painter = ui.painter();

    painter.circle_stroke(center, radius, Stroke::new(thickness, TRACK));
    if shown > 0.5 {
        let color = battery_color(shown);
        let sweep = (shown / 100.0).clamp(0.0, 1.0) * TAU;
        let steps = 72;
        let points: Vec<_> = (0..=steps)
            .map(|i| {
                let angle = -PI / 2.0 + sweep * (i as f32 / steps as f32);
                center + radius * vec2(angle.cos(), angle.sin())
            })
            .collect();
        painter.add(Shape::line(points.clone(), Stroke::new(thickness, color)));
        // Round the ends of the arc
        for end in [points.first(), points.last()].into_iter().flatten() {
            painter.circle_filled(*end, thickness / 2.0, color);
        }
    }
    let label = if percent.is_some() { format!("{}%", shown.round() as i64) } else { "–".to_string() };
    painter.text(center + vec2(0.0, -size * 0.04), Align2::CENTER_CENTER, label, FontId::proportional(size * 0.22), TEXT);
    painter.text(center + vec2(0.0, size * 0.13), Align2::CENTER_CENTER, "battery", FontId::proportional(size * 0.085), TEXT_WEAK);
}

/// A thin bar that animates to its value and changes color as it runs low.
pub fn meter(ui: &mut Ui, id: Id, label: &str, percent: i64) {
    let t = ui.ctx().animate_value_with_time(id, percent as f32, 0.8);
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(format!("{}%", t.round() as i64)).color(TEXT_WEAK));
        });
    });
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(5), TRACK);
    let mut fill = rect;
    fill.set_width((rect.width() * (t / 100.0).clamp(0.0, 1.0)).max(0.0));
    if fill.width() > 1.0 {
        painter.rect_filled(fill, CornerRadius::same(5), battery_color(t));
    }
    ui.add_space(4.0);
}

/// A sliding bar under the header while something is loading.
pub fn busy_bar(ui: &mut Ui, active: bool) {
    let t = ui.ctx().animate_bool_with_time(Id::unique("busy-bar"), active, 0.25);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 3.0), Sense::hover());
    if t <= 0.0 {
        return;
    }
    let time = ui.input(|i| i.time) as f32;
    let phase = (time * 0.9).fract();
    let width = rect.width() * 0.3;
    let x = egui::lerp((rect.left() - width)..=rect.right(), phase);
    let mut bar = rect;
    bar.min.x = x.max(rect.left());
    bar.max.x = (x + width).min(rect.right());
    if bar.width() > 0.0 {
        ui.painter().rect_filled(bar, CornerRadius::same(2), ACCENT.gamma_multiply(t));
    }
    ui.ctx().request_repaint();
}

/// A small rounded tag, e.g. a room name. It is one painted piece, so a row of them wraps cleanly.
pub fn chip(ui: &mut Ui, text: &str) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(14.0), TEXT);
    let pad = vec2(12.0, 6.0);
    let (rect, _) = ui.allocate_exact_size(galley.size() + pad * 2.0, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(14), Color32::from_rgb(34, 41, 54));
    painter.rect_stroke(rect, CornerRadius::same(14), Stroke::new(1.0, CARD_EDGE), egui::StrokeKind::Inside);
    painter.galley(rect.min + pad, galley, TEXT);
}
