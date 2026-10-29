//! The robot's map: floor-plan outline, room shapes and wall lines, as vector shapes.
//!
//! The robot sends them as base64 of a zstd-compressed JSON list. Coordinates are in the robot's
//! own units; dividing by 50 gives map pixels (the Ecovacs app does the same).

use anyhow::{Context, Result};
use base64::Engine;

/// Map units per pixel.
pub const UNITS_PER_PIXEL: f32 = 50.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    /// False when the shape lifts the pen before this point (a new sub-path starts here)
    pub connected: bool,
}

/// A closed shape: the floor outline, one room, or a wall line.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    /// For a room this is the room id
    pub id: String,
    pub points: Vec<Point>,
}

#[derive(Clone, Debug, Default)]
pub struct MapData {
    pub outlines: Vec<Shape>,
    pub rooms: Vec<Shape>,
    pub block_lines: Vec<Shape>,
    /// Entry types this reader doesn't understand yet, with how many entries each had
    pub other_entries: Vec<(String, usize)>,
}

impl MapData {
    /// The smallest box holding every point: (min x, min y, max x, max y), in map units.
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let points = self.outlines.iter().chain(&self.rooms).chain(&self.block_lines).flat_map(|s| &s.points);
        points.fold(None, |acc, p| match acc {
            None => Some((p.x, p.y, p.x, p.y)),
            Some((x0, y0, x1, y1)) => Some((x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y))),
        })
    }
}

/// Convert a map point to pixel coordinates, rotated the way the Ecovacs app shows the map.
/// `angle` is the map's rotation in degrees (0, 90, 180 or 270).
pub fn to_pixel(x: f32, y: f32, angle: i64) -> (f32, f32) {
    let (x, y) = (x / UNITS_PER_PIXEL, y / UNITS_PER_PIXEL);
    match angle.rem_euclid(360) {
        90 => (y, x),
        180 => (-x, y),
        270 => (-y, -x),
        _ => (x, -y),
    }
}

/// Convert a path point to pixel coordinates. Path points use units ten times finer than the
/// outline's, so the same rotation applies after scaling them up.
pub fn trace_to_pixel(x: f32, y: f32, angle: i64) -> (f32, f32) {
    to_pixel(x * 10.0, y * 10.0, angle)
}

/// The way the robot faces, from the angle it reports (degrees, counter-clockwise from +x in map
/// units), as a unit vector in map pixels. Unlike [`heading`] it is right while reversing too.
pub fn facing(robot_angle: f32, map_angle: i64) -> (f32, f32) {
    let radians = robot_angle.to_radians();
    let (x, y) = to_pixel(radians.cos(), radians.sin(), map_angle);
    let length = x.hypot(y);
    (x / length, y / length)
}

/// The direction the robot is travelling, as a unit vector in map pixels, taken from the end of
/// its path. Returns None when it has barely moved, or when the last stretch is a single point
/// (a jump). Works from the real path, so it needs no assumption about the robot's own angle value.
pub fn heading(trace: &[TracePoint], angle: i64) -> Option<(f32, f32)> {
    const MIN_DISTANCE: f32 = 1.5; // pixels; shorter hops are too noisy to give a direction
    let pixel = |p: &TracePoint| trace_to_pixel(p.x, p.y, angle);
    let last = pixel(trace.last()?);
    for pair in trace.windows(2).rev() {
        let (earlier, later) = (&pair[0], &pair[1]);
        if !later.connected {
            return None; // the path jumps here, so older points say nothing about the heading
        }
        let from = pixel(earlier);
        let (dx, dy) = (last.0 - from.0, last.1 - from.1);
        let length = dx.hypot(dy);
        if length >= MIN_DISTANCE {
            return Some((dx / length, dy / length));
        }
    }
    None
}

fn cross(o: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// Split a simple polygon into triangles (ear clipping), as index triples into `points`.
/// Handles concave shapes such as an L-shaped room. Bad input (a self-crossing outline) still
/// yields a best-effort result instead of failing.
pub fn triangulate(points: &[(f32, f32)]) -> Vec<[usize; 3]> {
    let n = points.len();
    if n < 3 {
        return Vec::new();
    }
    // Work counter-clockwise, whichever way the polygon was given
    let area: f32 = (0..n).map(|i| points[i].0 * points[(i + 1) % n].1 - points[(i + 1) % n].0 * points[i].1).sum();
    let mut order: Vec<usize> = if area >= 0.0 { (0..n).collect() } else { (0..n).rev().collect() };

    // A point on a triangle's edge counts as inside: an ear that touches another corner of the
    // polygon would spill outside it
    let inside = |p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        cross(a, b, p) >= -1e-6 && cross(b, c, p) >= -1e-6 && cross(c, a, p) >= -1e-6
    };

    let mut triangles = Vec::new();
    while order.len() > 3 {
        let m = order.len();
        let ear = (0..m).find(|&i| {
            let (a, b, c) = (order[(i + m - 1) % m], order[i], order[(i + 1) % m]);
            if cross(points[a], points[b], points[c]) <= 1e-6 {
                return false; // reflex or flat corner
            }
            !order.iter().any(|&other| {
                let p = points[other];
                // Corners that coincide with the triangle's own (repeated points) don't block it
                let own_corner = [a, b, c].iter().any(|&k| points[k] == p);
                !own_corner && inside(p, points[a], points[b], points[c])
            })
        });
        match ear {
            Some(i) => {
                triangles.push([order[(i + m - 1) % m], order[i], order[(i + 1) % m]]);
                order.remove(i);
            }
            None => {
                // No ear found (a degenerate outline): fan out what is left
                for k in 1..order.len() - 1 {
                    triangles.push([order[0], order[k], order[k + 1]]);
                }
                return triangles;
            }
        }
    }
    triangles.push([order[0], order[1], order[2]]);
    triangles
}

/// Decode base64 of a zstd frame into text.
pub(crate) fn unpack_zstd_base64(encoded: &str) -> Result<String> {
    let compressed = base64::engine::general_purpose::STANDARD.decode(encoded).context("data isn't valid base64")?;
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(&compressed[..])
        .map_err(|e| anyhow::anyhow!("data isn't valid zstd: {e}"))?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut decoder, &mut text).context("data couldn't be decompressed")?;
    Ok(text)
}

/// Read one shape string of the form `id;x,y[,kind];x,y[,kind];...`.
fn parse_shape(text: &str) -> Option<Shape> {
    let mut parts = text.split(';');
    let id = parts.next()?.trim().to_string();
    let points: Vec<Point> = parts
        .filter_map(|part| {
            let mut numbers = part.trim().split(',');
            let x = numbers.next()?.trim().parse().ok()?;
            let y = numbers.next()?.trim().parse().ok()?;
            // The third field, when present, marks where a new sub-path starts
            let connected = numbers.next().map_or(true, |kind| kind.trim() != "3-1-0");
            Some(Point { x, y, connected })
        })
        .collect();
    (!points.is_empty()).then_some(Shape { id, points })
}

/// One point of the path the robot has cleaned, in the robot's own units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TracePoint {
    pub x: f32,
    pub y: f32,
    /// False when the robot jumped here instead of driving (a new stretch of path starts)
    pub connected: bool,
}

/// Decode one page of the path: base64 of LZMA-compressed 5-byte records
/// (x and y as little-endian 16-bit numbers, then a flag byte whose top bit means "jump").
///
/// The robot shortens the standard LZMA header: it keeps the 5 property bytes and a 4-byte size
/// where the standard format has 8, so four zero bytes are added to make it readable.
pub fn decode_trace(encoded: &str) -> Result<Vec<TracePoint>> {
    const SHORT_HEADER: usize = 9;
    let short = base64::engine::general_purpose::STANDARD.decode(encoded).context("path data isn't valid base64")?;
    anyhow::ensure!(short.len() > SHORT_HEADER, "path data is too short");
    let mut compressed = Vec::with_capacity(short.len() + 4);
    compressed.extend_from_slice(&short[..SHORT_HEADER]);
    compressed.extend_from_slice(&[0, 0, 0, 0]);
    compressed.extend_from_slice(&short[SHORT_HEADER..]);

    let mut bytes = Vec::new();
    lzma_rs::lzma_decompress(&mut &compressed[..], &mut bytes).map_err(|e| anyhow::anyhow!("path data isn't valid LZMA: {e}"))?;
    anyhow::ensure!(bytes.len() % 5 == 0, "path data has a partial record ({} bytes)", bytes.len());
    Ok(bytes
        .chunks_exact(5)
        .map(|record| TracePoint {
            x: f32::from(i16::from_le_bytes([record[0], record[1]])),
            y: f32::from(i16::from_le_bytes([record[2], record[3]])),
            connected: record[4] >> 7 == 0,
        })
        .collect())
}

/// Decode the `info` field of a `getMapInfo_V2` reply.
pub fn decode_map_info(encoded: &str) -> Result<MapData> {
    let text = unpack_zstd_base64(encoded)?;
    let entries: Vec<Vec<String>> = serde_json::from_str(&text).context("map data isn't the expected list")?;

    let mut map = MapData::default();
    for entry in entries {
        let Some((kind, shapes)) = entry.split_first() else { continue };
        let parsed: Vec<Shape> = shapes.iter().filter_map(|s| parse_shape(s)).collect();
        match kind.as_str() {
            "1" => map.outlines.extend(parsed),
            // A room needs at least three points to enclose anything
            "2" => map.rooms.extend(parsed.into_iter().filter(|s| s.points.len() >= 3)),
            "6" => map.block_lines.extend(parsed.into_iter().filter(|s| s.points.len() >= 3)),
            other => match map.other_entries.iter_mut().find(|(k, _)| k == other) {
                Some((_, count)) => *count += shapes.len(),
                None => map.other_entries.push((other.to_string(), shapes.len())),
            },
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(json: &str) -> String {
        let compressed = ruzstd::encoding::compress_to_vec(json.as_bytes(), ruzstd::encoding::CompressionLevel::Fastest);
        base64::engine::general_purpose::STANDARD.encode(compressed)
    }

    #[test]
    fn decodes_outline_rooms_and_wall_lines() {
        let json = r#"[["1","0;0,0,0;100,0,0;100,100,0;0,100,0"],["2","3;10,10;50,10;50,50"],["6","9;1,1;2,2;3,3"],["5","x"],["4","y","z"]]"#;
        let map = decode_map_info(&encode(json)).unwrap();
        assert_eq!(map.outlines.len(), 1);
        assert_eq!(map.outlines[0].points.len(), 4);
        assert_eq!((map.rooms.len(), map.rooms[0].id.as_str()), (1, "3"));
        assert_eq!(map.block_lines.len(), 1);
        assert_eq!(map.other_entries, vec![("5".to_string(), 1), ("4".to_string(), 2)]);
        assert_eq!(map.bounds(), Some((0.0, 0.0, 100.0, 100.0)));
    }

    #[test]
    fn marks_a_new_sub_path_with_the_3_1_0_kind() {
        let map = decode_map_info(&encode(r#"[["1","0;0,0,0;10,0,3-1-0;10,10,0"]]"#)).unwrap();
        let connected: Vec<bool> = map.outlines[0].points.iter().map(|p| p.connected).collect();
        assert_eq!(connected, vec![true, false, true]);
    }

    #[test]
    fn rooms_with_fewer_than_three_points_are_dropped() {
        let map = decode_map_info(&encode(r#"[["2","1;0,0;5,5"]]"#)).unwrap();
        assert!(map.rooms.is_empty());
    }

    fn encode_trace(records: &[[u8; 5]]) -> String {
        let raw: Vec<u8> = records.iter().flatten().copied().collect();
        let mut compressed = Vec::new();
        // The robot records the real size, not "unknown"
        let options = lzma_rs::compress::Options {
            unpacked_size: lzma_rs::compress::UnpackedSize::WriteToHeader(Some(raw.len() as u64)),
        };
        lzma_rs::lzma_compress_with_options(&mut &raw[..], &mut compressed, &options).unwrap();
        // The robot's format has a 4-byte size field where the standard one has 8
        compressed.drain(9..13);
        base64::engine::general_purpose::STANDARD.encode(compressed)
    }

    #[test]
    fn decodes_path_records() {
        // (100, -50) driving, then (-2, 7) after a jump (top bit of the flag set)
        let encoded = encode_trace(&[[100, 0, 206, 255, 0], [254, 255, 7, 0, 0x80]]);
        let points = decode_trace(&encoded).unwrap();
        assert_eq!(
            points,
            vec![
                TracePoint { x: 100.0, y: -50.0, connected: true },
                TracePoint { x: -2.0, y: 7.0, connected: false },
            ]
        );
    }

    #[test]
    fn rejects_path_data_that_is_not_lzma() {
        assert!(decode_trace("aGVsbG8=").is_err());
    }

    fn area(points: &[(f32, f32)], triangles: &[[usize; 3]]) -> f32 {
        triangles.iter().map(|t| cross(points[t[0]], points[t[1]], points[t[2]]).abs() / 2.0).sum()
    }

    #[test]
    fn triangulates_a_square() {
        let square = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let triangles = triangulate(&square);
        assert_eq!(triangles.len(), 2);
        assert!((area(&square, &triangles) - 100.0).abs() < 1e-3);
    }

    #[test]
    fn triangulates_a_concave_l_shape_in_either_direction() {
        let l = [(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (10.0, 10.0), (10.0, 20.0), (0.0, 20.0)];
        let reversed: Vec<_> = l.iter().rev().copied().collect();
        for shape in [l.to_vec(), reversed] {
            let triangles = triangulate(&shape);
            assert_eq!(triangles.len(), 4);
            // The L is 300 square units; filling the notch would make it 400
            assert!((area(&shape, &triangles) - 300.0).abs() < 1e-3);
        }
    }

    #[test]
    fn too_few_points_make_no_triangles() {
        assert!(triangulate(&[(0.0, 0.0), (1.0, 1.0)]).is_empty());
    }

    fn pt(x: f32, y: f32, connected: bool) -> TracePoint {
        TracePoint { x, y, connected }
    }

    #[test]
    fn heading_follows_the_end_of_the_path() {
        // Driving along +x in path units; at 0 degrees that is +x on screen
        let path = [pt(0.0, 0.0, true), pt(10.0, 0.0, true), pt(20.0, 0.0, true)];
        assert_eq!(heading(&path, 0), Some((1.0, 0.0)));
        // Turned 180 degrees, the same drive points the other way on screen
        assert_eq!(heading(&path, 180), Some((-1.0, 0.0)));
    }

    #[test]
    fn facing_matches_the_path_direction() {
        // Driving along +x in path units is a robot angle of 0 degrees
        let (x, y) = facing(0.0, 0);
        assert!((x - 1.0).abs() < 1e-5 && y.abs() < 1e-5);
        let path = [pt(0.0, 0.0, true), pt(-9.0, 4.0, true), pt(-18.0, 8.0, true)];
        let (hx, hy) = heading(&path, 0).unwrap();
        let (fx, fy) = facing((4.0f32).atan2(-9.0).to_degrees(), 0);
        assert!((hx - fx).abs() < 1e-3 && (hy - fy).abs() < 1e-3);
    }

    #[test]
    fn heading_needs_real_movement_and_stops_at_a_jump() {
        assert_eq!(heading(&[], 0), None);
        assert_eq!(heading(&[pt(0.0, 0.0, true)], 0), None);
        // Too small a hop to give a direction
        assert_eq!(heading(&[pt(0.0, 0.0, true), pt(0.1, 0.0, true)], 0), None);
        // The last point is where the robot jumped to, so there is no direction yet
        assert_eq!(heading(&[pt(0.0, 0.0, true), pt(50.0, 0.0, true), pt(90.0, 90.0, false)], 0), None);
    }

    #[test]
    fn path_points_land_next_to_the_dock() {
        // Dock at (-678, 8) in map units; the first path point (-12, 0) in path units
        let dock = to_pixel(-678.0, 8.0, 0);
        let first = trace_to_pixel(-12.0, 0.0, 0);
        assert!((dock.0 - first.0).abs() < 15.0 && (dock.1 - first.1).abs() < 15.0, "{dock:?} vs {first:?}");
    }

    #[test]
    fn rotates_points_like_the_ecovacs_app() {
        assert_eq!(to_pixel(5000.0, 0.0, 0), (100.0, 0.0));
        assert_eq!(to_pixel(500.0, 1000.0, 0), (10.0, -20.0));
        assert_eq!(to_pixel(500.0, 1000.0, 180), (-10.0, 20.0));
        assert_eq!(to_pixel(500.0, 1000.0, 90), (20.0, 10.0));
        assert_eq!(to_pixel(500.0, 1000.0, 270), (-20.0, -10.0));
    }
}
