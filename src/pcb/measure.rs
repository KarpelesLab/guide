//! Measures of routes that lengths and pairs are judged by.

use crate::geom::Point;
use crate::geom::drc::Track;
use crate::id::NetId;

/// A track as straight pieces: itself, or an arc as chords within a few nanometres of it.
fn pieces(t: &Track) -> Vec<(Point, Point)> {
    let Some(mid) = t.mid else {
        return vec![(t.start, t.end)];
    };
    // The centre of the circle through the three points.
    let (ax, ay) = (t.start.x as f64, t.start.y as f64);
    let (bx, by) = (mid.x as f64, mid.y as f64);
    let (cx, cy) = (t.end.x as f64, t.end.y as f64);
    let d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
    if d == 0.0 {
        return vec![(t.start, mid), (mid, t.end)];
    }
    let (a2, b2, c2) = (ax * ax + ay * ay, bx * bx + by * by, cx * cx + cy * cy);
    let ox = (a2 * (by - cy) + b2 * (cy - ay) + c2 * (ay - by)) / d;
    let oy = (a2 * (cx - bx) + b2 * (ax - cx) + c2 * (bx - ax)) / d;
    let r = ((ax - ox) * (ax - ox) + (ay - oy) * (ay - oy)).sqrt();
    // Halving each chord by the point of the arc over its middle.
    let on_arc = |p: (f64, f64), q: (f64, f64)| {
        let (mx, my) = ((p.0 + q.0) / 2.0 - ox, (p.1 + q.1) / 2.0 - oy);
        let len = (mx * mx + my * my).sqrt();
        (ox + mx * r / len, oy + my * r / len)
    };
    let mut points = vec![(ax, ay), (bx, by), (cx, cy)];
    for _ in 0..5 {
        let mut finer = Vec::with_capacity(points.len() * 2);
        for w in points.windows(2) {
            finer.push(w[0]);
            finer.push(on_arc(w[0], w[1]));
        }
        finer.extend(points.last().copied());
        points = finer;
    }
    let round = |p: (f64, f64)| Point::new(p.0.round() as i64, p.1.round() as i64);
    points
        .windows(2)
        .map(|w| (round(w[0]), round(w[1])))
        .collect()
}

fn dist(a: Point, b: Point) -> f64 {
    let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
    (dx * dx + dy * dy).sqrt()
}

/// The length of a net's tracks, in nanometres: what a length constraint measures.
pub fn net_length(tracks: &[Track], net: NetId) -> f64 {
    tracks
        .iter()
        .filter(|t| t.net == Some(net))
        .flat_map(pieces)
        .map(|(a, b)| dist(a, b))
        .sum()
}

/// How much of a differential pair's positive net runs beside its negative net at the
/// pair's gap: the length of the positive net's tracks, and of that, the length where the
/// negative net's nearest track on the same layer is `gap` from it, edge to edge, give or
/// take `tolerance`. Measured every 10 µm along the tracks.
pub fn coupling(
    tracks: &[Track],
    positive: NetId,
    negative: NetId,
    gap: i64,
    tolerance: i64,
) -> (f64, f64) {
    const STEP: f64 = 10_000.0;
    let others: Vec<(Point, Point, u8, i64)> = tracks
        .iter()
        .filter(|t| t.net == Some(negative))
        .flat_map(|t| {
            pieces(t)
                .into_iter()
                .map(move |(a, b)| (a, b, t.layer.0, t.width))
        })
        .collect();
    let (mut total, mut coupled) = (0.0, 0.0);
    for t in tracks.iter().filter(|t| t.net == Some(positive)) {
        for (a, b) in pieces(t) {
            let len = dist(a, b);
            total += len;
            let n = (len / STEP).ceil().max(1.0) as usize;
            for i in 0..n {
                let f = (i as f64 + 0.5) / n as f64;
                let p = (
                    a.x as f64 + (b.x - a.x) as f64 * f,
                    a.y as f64 + (b.y - a.y) as f64 * f,
                );
                let nearest = others
                    .iter()
                    .filter(|o| o.2 == t.layer.0)
                    .map(|&(c, d, _, w)| segment_distance(p, c, d) - (w + t.width) as f64 / 2.0)
                    .fold(f64::INFINITY, f64::min);
                if (nearest - gap as f64).abs() <= tolerance as f64 {
                    coupled += len / n as f64;
                }
            }
        }
    }
    (coupled, total)
}

/// The distance from a point to a segment.
fn segment_distance(p: (f64, f64), a: Point, b: Point) -> f64 {
    let (ax, ay) = (a.x as f64, a.y as f64);
    let (dx, dy) = (b.x as f64 - ax, b.y as f64 - ay);
    let len2 = dx * dx + dy * dy;
    let u = if len2 == 0.0 {
        0.0
    } else {
        (((p.0 - ax) * dx + (p.1 - ay) * dy) / len2).clamp(0.0, 1.0)
    };
    let (x, y) = (ax + u * dx - p.0, ay + u * dy - p.1);
    (x * x + y * y).sqrt()
}
