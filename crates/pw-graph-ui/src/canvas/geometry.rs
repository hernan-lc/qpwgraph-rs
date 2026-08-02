//! Bezier link curves and hit-testing math, independent of egui's widget
//! system so it stays easy to unit test on its own.

use egui::{pos2, vec2, Pos2, Rect};

pub(crate) fn bezier_points(start: Pos2, end: Pos2, lane_offset: f32) -> Vec<Pos2> {
    let handle = (end.x - start.x).abs().clamp(72.0, 220.0) * 0.45;
    let control_one = start + vec2(handle, lane_offset);
    let control_two = end - vec2(handle, lane_offset);
    (0..=28)
        .map(|index| {
            let t = index as f32 / 28.0;
            let inverse = 1.0 - t;
            let point = start.to_vec2() * inverse.powi(3)
                + control_one.to_vec2() * (3.0 * inverse.powi(2) * t)
                + control_two.to_vec2() * (3.0 * inverse * t.powi(2))
                + end.to_vec2() * t.powi(3);
            pos2(point.x, point.y)
        })
        .collect()
}

pub(crate) fn points_bounds(points: &[Pos2]) -> Rect {
    let mut min = points[0];
    let mut max = points[0];
    for point in points.iter().skip(1) {
        min.x = min.x.min(point.x);
        min.y = min.y.min(point.y);
        max.x = max.x.max(point.x);
        max.y = max.y.max(point.y);
    }
    Rect::from_min_max(min, max)
}

pub(crate) fn point_near_polyline(point: Pos2, points: &[Pos2], threshold: f32) -> bool {
    let threshold_squared = threshold * threshold;
    points.windows(2).any(|segment| {
        point_to_segment_distance_squared(point, segment[0], segment[1]) <= threshold_squared
    })
}

fn point_to_segment_distance_squared(point: Pos2, start: Pos2, end: Pos2) -> f32 {
    let segment = end - start;
    let length_squared = segment.length_sq();
    if length_squared <= f32::EPSILON {
        return point.distance_sq(start);
    }
    let factor = ((point - start).dot(segment) / length_squared).clamp(0.0, 1.0);
    point.distance_sq(start + segment * factor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bezier_curve_starts_and_ends_at_ports() {
        let start = pos2(20.0, 40.0);
        let end = pos2(220.0, 120.0);
        let points = bezier_points(start, end, 0.0);
        assert_eq!(points.first().copied(), Some(start));
        assert_eq!(points.last().copied(), Some(end));
        assert!(point_near_polyline(points[14], &points, 1.0));
    }
}
