// the cloverleaf mark: four heart-shaped leaves and a stem on a rounded accent tile.
// drawn from primitives so it scales to any size and can spin (the leaves double as the spinner).
// assets/cloverleaf.svg is the same geometry for use outside the app.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};

fn rot(v: Vec2, a: f32) -> Vec2 {
    let (s, c) = a.sin_cos();
    vec2(v.x * c - v.y * s, v.x * s + v.y * c)
}

// full logo: tile + clover. `angle` spins the clover (radians).
pub fn paint(pt: &Painter, rect: Rect, tile: Color32, leaf: Color32, angle: f32) {
    let s = rect.width().min(rect.height());
    let sq = Rect::from_center_size(rect.center(), vec2(s, s));
    pt.rect_filled(sq, s * 0.25, tile);
    // sits a bit above center so the stem has room underneath.
    paint_clover(pt, pos2(sq.center().x, sq.top() + s * 0.46), s * 0.70, leaf, angle, true);
}

// just the clover. `size` is roughly the diameter of the leaves.
pub fn paint_clover(pt: &Painter, c: Pos2, size: f32, col: Color32, angle: f32, stem: bool) {
    let len = size * 0.52; // center to leaf tip
    let r = len * 0.25; // lobe radius
    for i in 0..4 {
        // leaves point along the diagonals.
        let d = rot(vec2(1.0, 0.0), angle + std::f32::consts::FRAC_PI_4 + i as f32 * std::f32::consts::FRAC_PI_2);
        let n = vec2(-d.y, d.x);
        // a heart: two round lobes at the tip, tapering to a point near the center.
        let base = c + d * (len * 0.14);
        let a = c + d * (len - r) + n * (r * 0.92);
        let b = c + d * (len - r) - n * (r * 0.92);
        let ta = a + n * (r * 0.78) - d * (r * 0.62);
        let tb = b - n * (r * 0.78) - d * (r * 0.62);
        pt.circle_filled(a, r, col);
        pt.circle_filled(b, r, col);
        pt.add(Shape::convex_polygon(vec![base, ta, a, b, tb], col, Stroke::NONE));
    }
    if stem {
        // down through the gap between the two bottom leaves, flicking right at the end.
        let p1 = c + rot(vec2(0.0, len * 1.34 * 0.72), angle);
        let p2 = c + rot(vec2(len * 0.22, len * 1.34), angle);
        let stroke = Stroke::new((size * 0.062).max(1.0), col);
        pt.add(egui::epaint::QuadraticBezierShape::from_points_stroke([c, p1, p2], false, Color32::TRANSPARENT, stroke));
    }
}
