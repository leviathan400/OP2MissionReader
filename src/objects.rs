//! Drawing the mission's objects on the map: their art when tiles are big enough to see it, coloured dots when
//! zoomed out (or without art); and the overlays the View menu's windows ask for: trigger areas, AI groups and their
//! plans, forecast disasters and replayed messages. Places are file tiles, 32 map pixels each, put on screen through
//! the viewport's [`Transform`].

use eframe::egui::{self, Color32, Rect, Stroke, Vec2, pos2, vec2};

use crate::mapview::Transform;
use crate::setup::{Kind, Object, Setup};
use crate::textures::ArtTextures;

/// Below this many screen pixels per tile, objects are dots.
const ART_MIN_TILE_PX: f32 = 8.0;

/// Shared with the whole-map export (`scene`), so the window and a saved image show the same objects the same way.
pub use crate::scene::{Layers, color_name, plan};

/// The player colour as `scene::player_rgb` gives it.
pub fn player_color(color: Option<u8>) -> Color32 {
    let [r, g, b] = crate::scene::player_rgb(color);
    Color32::from_rgb(r, g, b)
}

/// A dot's fill, as `scene::dot_rgb` gives it.
fn dot_color(o: &Object) -> Color32 {
    let [r, g, b] = crate::scene::dot_rgb(o);
    Color32::from_rgb(r, g, b)
}

/// Draws `setup`'s objects and returns the one under the mouse (the topmost).
pub fn draw(ctx: &egui::Context, painter: &egui::Painter, t: &Transform, setup: &Setup, mut art: Option<&mut ArtTextures>, layers: &Layers) -> Option<u32> {
    let use_art = layers.art && t.tile_px() >= ART_MIN_TILE_PX;
    let mut hovered = None;
    // In `plan`'s drawing order, so the last object under the mouse is the one drawn on top.
    for (o, look) in plan(setup, layers) {
        let drawn = match (use_art, art.as_deref_mut(), look) {
            // `at` is the map pixel the art anchors to; the frame's hotspot sits on it.
            (true, Some(art), Some((look, at))) => art.get(ctx, &look).map(|d| {
                let r = Rect::from_min_size(t.to_screen(vec2(at.0, at.1) - d.hotspot), d.size * t.zoom);
                painter.image(d.texture.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                r
            }),
            _ => None,
        };
        // A dot too when the art for this look didn't load.
        let r = drawn.unwrap_or_else(|| dot(painter, t, o));
        if t.pointer.is_some_and(|p| r.contains(p)) {
            hovered = Some(o.id);
        }
    }
    hovered
}

/// The tiles rectangle, point and escape triggers watch, outlined with their trigger number: all of them, or only
/// `selected` (drawn brighter either way).
pub fn draw_areas(painter: &egui::Painter, t: &Transform, setup: &Setup, selected: Option<u32>, all: bool) {
    for tr in setup.triggers.iter().filter(|tr| all || selected == Some(tr.id)) {
        let Some(((x1, y1), (x2, y2))) = tr.area else { continue };
        let min = t.to_screen(vec2(x1 as f32, y1 as f32) * 32.0);
        let max = t.to_screen(vec2((x2 + 1) as f32, (y2 + 1) as f32) * 32.0);
        let r = Rect::from_two_pos(min, max);
        let (color, width) = if selected == Some(tr.id) { (Color32::from_rgb(255, 240, 80), 3.0) } else { (Color32::from_rgb(255, 160, 40), 1.5) };
        painter.rect(r, 0.0, color.gamma_multiply(0.12), Stroke::new(width, color), egui::StrokeKind::Inside);
        painter.text(r.min + vec2(3.0, 2.0), egui::Align2::LEFT_TOP, format!("#{}", tr.id), egui::FontId::proportional(12.0), color);
    }
}

/// The AI groups on the map: `selected`'s areas, patrol route and units in cyan, and its plans (every building
/// group's with `all_plans`) as dashed outlines, green where nothing stands yet.
pub fn draw_groups(painter: &egui::Painter, t: &Transform, setup: &Setup, selected: Option<u32>, all_plans: bool) {
    let rect = |((x1, y1), (x2, y2)): crate::groups::Area| {
        Rect::from_two_pos(t.to_screen(vec2(x1 as f32, y1 as f32) * 32.0), t.to_screen(vec2((x2 + 1) as f32, (y2 + 1) as f32) * 32.0))
    };
    let font = egui::FontId::proportional(11.0);
    let cyan = Color32::from_rgb(80, 220, 255);
    for g in &setup.groups {
        let chosen = selected == Some(g.id);
        if !(chosen || all_plans) {
            continue;
        }
        for p in &g.plans {
            let r = rect(p.area);
            let color = if p.standing { Color32::from_rgb(170, 200, 170) } else { Color32::from_rgb(90, 240, 90) };
            // A planned wall or tube tile: a small filled square in the tile's middle.
            if p.wall {
                painter.rect_filled(r.shrink(r.width() * 0.3), 0.0, color.gamma_multiply(0.7));
                continue;
            }
            let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
            painter.extend(egui::Shape::dashed_line(&corners, Stroke::new(if chosen { 2.0 } else { 1.5 }, color), 6.0, 4.0));
            if t.tile_px() >= 12.0 {
                // The plan's first word: its structure or mine code.
                let code = p.what.split([' ', ',']).next().unwrap_or_default();
                painter.text(r.min + vec2(3.0, 2.0), egui::Align2::LEFT_TOP, code, font.clone(), color);
            }
        }
        if !chosen {
            continue;
        }
        // Waypoints are dots joined into the patrol route, in order; every other area is an outlined rectangle.
        let mut route = Vec::new();
        for (label, area) in &g.areas {
            let r = rect(*area);
            if label.starts_with("waypoint") {
                route.push(r.center());
                painter.circle(r.center(), 5.0, cyan, Stroke::new(1.0, Color32::BLACK));
            } else {
                painter.rect(r, 0.0, cyan.gamma_multiply(0.10), Stroke::new(2.0, cyan), egui::StrokeKind::Inside);
                painter.text(r.min + vec2(3.0, 2.0), egui::Align2::LEFT_TOP, format!("#{} {label}", g.id), egui::FontId::proportional(12.0), cyan);
            }
        }
        if route.len() > 1 {
            painter.line(route, Stroke::new(1.5, cyan));
        }
        for o in g.members.iter().filter_map(|&id| setup.objects.iter().find(|o| o.id == id)) {
            painter.rect_stroke(rect(o.footprint), 2.0, Stroke::new(2.0, cyan), egui::StrokeKind::Outside);
        }
    }
}

/// The forecast disasters with a known place: a red ring with the disaster's initial at the tile it strikes (a storm's
/// or vortex's path to where it heads, a blight area outlined), `selected` brighter and labelled with its time.
pub fn draw_disasters(painter: &egui::Painter, t: &Transform, f: &crate::disasters::Forecast, selected: Option<usize>) {
    let centre = |(x, y): (i32, i32)| t.to_screen((vec2(x as f32, y as f32) + Vec2::splat(0.5)) * 32.0);
    for (i, d) in f.disasters.iter().enumerate() {
        let Some(at) = d.at else { continue };
        let chosen = selected == Some(i);
        let color = if chosen { Color32::from_rgb(255, 240, 80) } else { Color32::from_rgb(255, 80, 60) };
        let stroke = Stroke::new(if chosen { 3.0 } else { 2.0 }, color);
        match d.end_at {
            Some(end) if d.kind == "BLIGHT" => {
                let r = Rect::from_two_pos(t.to_screen(vec2(at.0 as f32, at.1 as f32) * 32.0), t.to_screen(vec2((end.0 + 1) as f32, (end.1 + 1) as f32) * 32.0));
                painter.rect(r, 0.0, color.gamma_multiply(0.12), stroke, egui::StrokeKind::Inside);
            }
            Some(end) => painter.arrow(centre(at), centre(end) - centre(at), stroke),
            None => {}
        }
        let radius = (t.tile_px() * 0.6).max(9.0);
        painter.circle(centre(at), radius, Color32::from_black_alpha(140), stroke);
        let initial = d.name().chars().next().unwrap_or('?').to_string();
        painter.text(centre(at), egui::Align2::CENTER_CENTER, initial, egui::FontId::proportional(radius * 1.1), color);
        if chosen {
            painter.text(centre(at) + vec2(radius + 4.0, 0.0), egui::Align2::LEFT_CENTER, format!("{}, {}", d.name(), d.when()), egui::FontId::proportional(13.0), color);
        }
    }
}

/// The replayed messages that point at a place: a blue ring there, `selected` brighter and labelled with its text.
pub fn draw_messages(painter: &egui::Painter, t: &Transform, m: &crate::messages::Messages, selected: Option<usize>) {
    for (i, msg) in m.messages.iter().enumerate() {
        let Some((x, y)) = msg.at else { continue };
        let chosen = selected == Some(i);
        let color = if chosen { Color32::from_rgb(255, 240, 80) } else { Color32::from_rgb(90, 160, 255) };
        let centre = t.to_screen((vec2(x as f32, y as f32) + Vec2::splat(0.5)) * 32.0);
        let radius = (t.tile_px() * 0.5).max(7.0);
        painter.circle_stroke(centre, radius, Stroke::new(if chosen { 3.0 } else { 2.0 }, color));
        if chosen {
            painter.text(centre + vec2(radius + 4.0, 0.0), egui::Align2::LEFT_CENTER, format!("{}: {}", msg.when(), msg.text), egui::FontId::proportional(13.0), color);
        }
    }
}

/// A marker for an object without art (or when zoomed out): a square for a structure, a circle for anything else.
fn dot(painter: &egui::Painter, t: &Transform, o: &Object) -> Rect {
    let fill = dot_color(o);
    let edge = Stroke::new(1.0, Color32::BLACK);
    // The tile's centre, wherever the art would anchor.
    let centre = t.to_screen(vec2(o.tile.0 as f32, o.tile.1 as f32) * 32.0 + Vec2::splat(16.0));
    if o.kind == Kind::Structure {
        let r = Rect::from_center_size(centre, Vec2::splat((t.tile_px() * 0.8).max(5.0)));
        painter.rect(r, 1.0, fill, edge, egui::StrokeKind::Outside);
        r
    } else {
        let radius = (t.tile_px() * 0.3).max(2.5);
        painter.circle(centre, radius, fill, edge);
        Rect::from_center_size(centre, Vec2::splat(radius * 2.0))
    }
}
