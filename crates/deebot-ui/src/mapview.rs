//! Draws the floor plan, rooms, cleaning path, dock and robot.

use deebot_core::map::{facing, to_pixel, trace_to_pixel, triangulate, Shape, TracePoint};
use deebot_core::robot::{MapView, Positions, Room};
use eframe::egui::{self, vec2, Align2, Color32, CornerRadius, FontId, Mesh, Pos2, Sense, Stroke, Ui};

use crate::theme::{ACCENT, BAD, TEXT, WARN};

/// Soft fill colors for the rooms, repeated if there are more rooms than colors.
const ROOM_COLORS: [Color32; 8] = [
    Color32::from_rgb(78, 121, 167),
    Color32::from_rgb(242, 142, 43),
    Color32::from_rgb(89, 161, 79),
    Color32::from_rgb(176, 122, 161),
    Color32::from_rgb(237, 201, 72),
    Color32::from_rgb(118, 183, 178),
    Color32::from_rgb(225, 87, 89),
    Color32::from_rgb(156, 117, 95),
];

/// What changes while the robot cleans.
pub struct Live<'a> {
    pub positions: &'a Positions,
    pub trace: &'a [TracePoint],
    pub cleaning: bool,
}

/// Split a shape into runs of connected points, so a jump starts a new line.
fn runs(shape: &Shape) -> Vec<Vec<(f32, f32)>> {
    let mut runs: Vec<Vec<(f32, f32)>> = Vec::new();
    for point in &shape.points {
        if !point.connected || runs.is_empty() {
            runs.push(Vec::new());
        }
        if let Some(run) = runs.last_mut() {
            run.push((point.x, point.y));
        }
    }
    runs
}

pub fn draw(ui: &mut Ui, view: &MapView, room_names: &[Room], live: Live<'_>) {
    let Some((x0, y0, x1, y1)) = view.map.bounds() else {
        ui.label("The map is empty.");
        return;
    };

    // How much room the map needs once it is turned the way the Ecovacs app shows it
    let corners = [(x0, y0), (x1, y0), (x0, y1), (x1, y1)].map(|(x, y)| to_pixel(x, y, view.angle));
    let min_x = corners.iter().map(|c| c.0).fold(f32::MAX, f32::min);
    let max_x = corners.iter().map(|c| c.0).fold(f32::MIN, f32::max);
    let min_y = corners.iter().map(|c| c.1).fold(f32::MAX, f32::min);
    let max_y = corners.iter().map(|c| c.1).fold(f32::MIN, f32::max);
    let margin = 6.0;
    let (width, height) = (max_x - min_x + 2.0 * margin, max_y - min_y + 2.0 * margin);

    let available = ui.available_width();
    let scale = (available / width).min(560.0 / height);
    let (rect, _) = ui.allocate_exact_size(vec2(available, height * scale), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(12), Color32::from_rgb(16, 20, 28));

    // Map pixels to screen points, centered when the map is the narrower side
    let left = rect.left() + (available - width * scale) / 2.0;
    let project = |(px, py): (f32, f32)| Pos2::new(left + (px - min_x + margin) * scale, rect.top() + (py - min_y + margin) * scale);
    let on_map = |x: f32, y: f32| project(to_pixel(x, y, view.angle));

    // Rooms: filled, outlined and named
    for (index, room) in view.map.rooms.iter().enumerate() {
        let points: Vec<Pos2> = room.points.iter().map(|p| on_map(p.x, p.y)).collect();
        let flat: Vec<(f32, f32)> = points.iter().map(|p| (p.x, p.y)).collect();
        let fill = ROOM_COLORS[index % ROOM_COLORS.len()].gamma_multiply(0.42);
        let mut mesh = Mesh::default();
        for point in &points {
            mesh.colored_vertex(*point, fill);
        }
        for triangle in triangulate(&flat) {
            mesh.add_triangle(triangle[0] as u32, triangle[1] as u32, triangle[2] as u32);
        }
        painter.add(egui::Shape::mesh(mesh));
        painter.add(egui::Shape::closed_line(points.clone(), Stroke::new(1.0, ROOM_COLORS[index % ROOM_COLORS.len()].gamma_multiply(0.8))));

        let name = room.id.parse::<i64>().ok().and_then(|id| room_names.iter().find(|r| r.id == id)).map(|r| r.label().to_string());
        if let Some(name) = name {
            let (lo_x, hi_x) = points.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.x), hi.max(p.x)));
            let (lo_y, hi_y) = points.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.y), hi.max(p.y)));
            painter.text(
                Pos2::new((lo_x + hi_x) / 2.0, (lo_y + hi_y) / 2.0),
                Align2::CENTER_CENTER,
                name,
                FontId::proportional(12.0),
                TEXT.gamma_multiply(0.9),
            );
        }
    }

    // Walls: the floor-plan outline, and the robot's blocked lines
    for outline in &view.map.outlines {
        for run in runs(outline) {
            let points: Vec<Pos2> = run.iter().map(|&(x, y)| on_map(x, y)).collect();
            let closed = outline.points.iter().all(|p| p.connected);
            let stroke = Stroke::new(1.6, Color32::from_rgb(190, 198, 212));
            painter.add(if closed { egui::Shape::closed_line(points, stroke) } else { egui::Shape::line(points, stroke) });
        }
    }
    for line in &view.map.block_lines {
        let points: Vec<Pos2> = line.points.iter().map(|p| on_map(p.x, p.y)).collect();
        painter.add(egui::Shape::closed_line(points, Stroke::new(1.4, BAD.gamma_multiply(0.85))));
    }

    // The path the robot has driven
    let mut segment: Vec<Pos2> = Vec::new();
    for point in live.trace {
        if !point.connected && segment.len() > 1 {
            painter.add(egui::Shape::line(std::mem::take(&mut segment), Stroke::new(2.0, ACCENT)));
        }
        if !point.connected {
            segment.clear();
        }
        segment.push(project(trace_to_pixel(point.x, point.y, view.angle)));
    }
    if segment.len() > 1 {
        painter.add(egui::Shape::line(segment, Stroke::new(2.0, ACCENT)));
    }

    // The dock, and the robot on top
    if let Some(dock) = live.positions.charger {
        let center = on_map(dock.x, dock.y);
        let diamond = vec![
            center + vec2(0.0, -7.0),
            center + vec2(7.0, 0.0),
            center + vec2(0.0, 7.0),
            center + vec2(-7.0, 0.0),
        ];
        painter.add(egui::Shape::convex_polygon(diamond, WARN, Stroke::new(1.0, Color32::WHITE)));
    }
    if let Some(robot) = live.positions.robot {
        let center = on_map(robot.x, robot.y);
        if live.cleaning {
            let wave = ((ui.input(|i| i.time) as f32) * 3.0).sin() * 0.5 + 0.5;
            painter.circle_filled(center, 8.0 + 8.0 * wave, ACCENT.gamma_multiply(0.35 * (1.0 - wave) + 0.08));
            ui.ctx().request_repaint();
        }
        // An arrow along the way it faces, drawn under the dot so the tip pokes out
        if live.cleaning {
            let (dx, dy) = facing(robot.angle, view.angle);
            let (forward, side) = (vec2(dx, dy), vec2(-dy, dx));
            let arrow = vec![center + forward * 16.0, center + side * 6.5 - forward, center - side * 6.5 - forward];
            painter.add(egui::Shape::convex_polygon(arrow, Color32::WHITE, Stroke::new(1.0, ACCENT)));
        }
        painter.circle_filled(center, 7.0, ACCENT);
        painter.circle_stroke(center, 7.0, Stroke::new(2.0, Color32::WHITE));
    }
}
