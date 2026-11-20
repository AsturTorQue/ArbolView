//! Squarified tree map layout.

use egui::{pos2, Rect};

fn worst(row: &[f64], side: f64) -> f64 {
    let s: f64 = row.iter().sum();
    if s <= 0.0 || side <= 0.0 {
        return f64::INFINITY;
    }
    let mx = row.iter().cloned().fold(0.0, f64::max);
    let mn = row.iter().cloned().fold(f64::INFINITY, f64::min);
    let s2 = s * s;
    let w2 = side * side;
    f64::max(w2 * mx / s2, s2 / (w2 * mn))
}

/// Lay out `values` (sorted large to small) inside `rect`; returns one rect per value.
pub fn layout(values: &[f64], rect: Rect) -> Vec<Rect> {
    let mut out = vec![Rect::NOTHING; values.len()];
    let total: f64 = values.iter().sum();
    if total <= 0.0 || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return out;
    }
    let scale = (rect.width() as f64 * rect.height() as f64) / total;
    let areas: Vec<f64> = values.iter().map(|v| v * scale).collect();
    let mut free = rect;
    let mut i = 0;
    while i < areas.len() {
        let side = f64::min(free.width() as f64, free.height() as f64);
        let mut j = i + 1;
        while j < areas.len() && worst(&areas[i..j + 1], side) <= worst(&areas[i..j], side) {
            j += 1;
        }
        let row = &areas[i..j];
        let s: f64 = row.iter().sum();
        if free.width() >= free.height() {
            let w = (s / free.height() as f64) as f32;
            let mut y = free.top();
            for (k, a) in row.iter().enumerate() {
                let h = (*a / w as f64) as f32;
                out[i + k] = Rect::from_min_max(pos2(free.left(), y), pos2(free.left() + w, y + h));
                y += h;
            }
            free = Rect::from_min_max(pos2(free.left() + w, free.top()), free.max);
        } else {
            let h = (s / free.width() as f64) as f32;
            let mut x = free.left();
            for (k, a) in row.iter().enumerate() {
                let w = (*a / h as f64) as f32;
                out[i + k] = Rect::from_min_max(pos2(x, free.top()), pos2(x + w, free.top() + h));
                x += w;
            }
            free = Rect::from_min_max(pos2(free.left(), free.top() + h), free.max);
        }
        i = j;
    }
    out
}
