//! Cell-exact drawing for box drawing, block elements, Braille and the
//! dentistry corners `⎾ ⎿`, in place of font glyphs. These must tile across
//! neighbouring cells, which font glyphs rarely do, and no bundled font has
//! Braille. Like Ghostty's sprite font and kitty's box drawing, geometry is
//! whole device pixels, so lines stay crisp and meet their neighbours.
use bedtime_ui::egui::{Color32, Mesh, Painter, Pos2, Rect, Shape, Stroke, pos2};

/// `text`'s character, when it is one character drawn here rather than shaped.
pub(crate) fn drawn(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let c = chars.next()?;
    let covered = matches!(
        c,
        '\u{2500}'..='\u{259F}' | '\u{2800}'..='\u{28FF}' | '\u{23BE}' | '\u{23BF}'
    );
    (covered && chars.next().is_none()).then_some(c)
}

/// Draw `c` filling `rect` (a whole cell span), snapped to device pixels.
pub(crate) fn paint(painter: &Painter, c: char, rect: Rect, color: Color32, density: f32) {
    let x0 = (rect.left() * density).round();
    let y0 = (rect.top() * density).round();
    let w = (rect.right() * density).round() - x0;
    let h = (rect.bottom() * density).round() - y0;
    if w < 1.0 || h < 1.0 {
        return;
    }
    let at = |x: f32, y: f32| pos2((x0 + x) / density, (y0 + y) / density);
    let mut mesh = Mesh::default();
    for piece in pieces(c, w as i32, h as i32) {
        match piece {
            Piece::Fill { x, y, alpha } => mesh.add_colored_rect(
                Rect::from_min_max(at(x.0 as f32, y.0 as f32), at(x.1 as f32, y.1 as f32)),
                if alpha == 255 {
                    color
                } else {
                    color.gamma_multiply(f32::from(alpha) / 255.0)
                },
            ),
            Piece::Line { points, width } => {
                let points: Vec<Pos2> = points.into_iter().map(|[x, y]| at(x, y)).collect();
                painter.add(Shape::line(points, Stroke::new(width / density, color)));
            }
        }
    }
    if !mesh.is_empty() {
        painter.add(Shape::mesh(mesh));
    }
}

#[derive(Debug, PartialEq)]
enum Piece {
    /// Device pixels `[x.0, x.1) × [y.0, y.1)`, at `alpha` of the colour.
    Fill {
        x: (i32, i32),
        y: (i32, i32),
        alpha: u8,
    },
    /// An anti-aliased polyline through device-pixel points.
    Line { points: Vec<[f32; 2]>, width: f32 },
}

fn fill(x: (i32, i32), y: (i32, i32)) -> Piece {
    Piece::Fill { x, y, alpha: 255 }
}

/// Arm weights `[up, right, down, left]` for U+2500–257F: 0 none, 1 light,
/// 2 heavy, 3 double. Dashes, arcs and diagonals are zero here and drawn
/// separately. Generated from the Unicode character names.
#[rustfmt::skip]
const BOX: [[u8; 4]; 128] = [
    // ─ ━ │ ┃ ┄ ┅ ┆ ┇
    [0, 1, 0, 1], [0, 2, 0, 2], [1, 0, 1, 0], [2, 0, 2, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0],
    // ┈ ┉ ┊ ┋ ┌ ┍ ┎ ┏
    [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 1, 1, 0], [0, 2, 1, 0], [0, 1, 2, 0], [0, 2, 2, 0],
    // ┐ ┑ ┒ ┓ └ ┕ ┖ ┗
    [0, 0, 1, 1], [0, 0, 1, 2], [0, 0, 2, 1], [0, 0, 2, 2], [1, 1, 0, 0], [1, 2, 0, 0], [2, 1, 0, 0], [2, 2, 0, 0],
    // ┘ ┙ ┚ ┛ ├ ┝ ┞ ┟
    [1, 0, 0, 1], [1, 0, 0, 2], [2, 0, 0, 1], [2, 0, 0, 2], [1, 1, 1, 0], [1, 2, 1, 0], [2, 1, 1, 0], [1, 1, 2, 0],
    // ┠ ┡ ┢ ┣ ┤ ┥ ┦ ┧
    [2, 1, 2, 0], [2, 2, 1, 0], [1, 2, 2, 0], [2, 2, 2, 0], [1, 0, 1, 1], [1, 0, 1, 2], [2, 0, 1, 1], [1, 0, 2, 1],
    // ┨ ┩ ┪ ┫ ┬ ┭ ┮ ┯
    [2, 0, 2, 1], [2, 0, 1, 2], [1, 0, 2, 2], [2, 0, 2, 2], [0, 1, 1, 1], [0, 1, 1, 2], [0, 2, 1, 1], [0, 2, 1, 2],
    // ┰ ┱ ┲ ┳ ┴ ┵ ┶ ┷
    [0, 1, 2, 1], [0, 1, 2, 2], [0, 2, 2, 1], [0, 2, 2, 2], [1, 1, 0, 1], [1, 1, 0, 2], [1, 2, 0, 1], [1, 2, 0, 2],
    // ┸ ┹ ┺ ┻ ┼ ┽ ┾ ┿
    [2, 1, 0, 1], [2, 1, 0, 2], [2, 2, 0, 1], [2, 2, 0, 2], [1, 1, 1, 1], [1, 1, 1, 2], [1, 2, 1, 1], [1, 2, 1, 2],
    // ╀ ╁ ╂ ╃ ╄ ╅ ╆ ╇
    [2, 1, 1, 1], [1, 1, 2, 1], [2, 1, 2, 1], [2, 1, 1, 2], [2, 2, 1, 1], [1, 1, 2, 2], [1, 2, 2, 1], [2, 2, 1, 2],
    // ╈ ╉ ╊ ╋ ╌ ╍ ╎ ╏
    [1, 2, 2, 2], [2, 1, 2, 2], [2, 2, 2, 1], [2, 2, 2, 2], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0],
    // ═ ║ ╒ ╓ ╔ ╕ ╖ ╗
    [0, 3, 0, 3], [3, 0, 3, 0], [0, 3, 1, 0], [0, 1, 3, 0], [0, 3, 3, 0], [0, 0, 1, 3], [0, 0, 3, 1], [0, 0, 3, 3],
    // ╘ ╙ ╚ ╛ ╜ ╝ ╞ ╟
    [1, 3, 0, 0], [3, 1, 0, 0], [3, 3, 0, 0], [1, 0, 0, 3], [3, 0, 0, 1], [3, 0, 0, 3], [1, 3, 1, 0], [3, 1, 3, 0],
    // ╠ ╡ ╢ ╣ ╤ ╥ ╦ ╧
    [3, 3, 3, 0], [1, 0, 1, 3], [3, 0, 3, 1], [3, 0, 3, 3], [0, 3, 1, 3], [0, 1, 3, 1], [0, 3, 3, 3], [1, 3, 0, 3],
    // ╨ ╩ ╪ ╫ ╬ ╭ ╮ ╯
    [3, 1, 0, 1], [3, 3, 0, 3], [1, 3, 1, 3], [3, 1, 3, 1], [3, 3, 3, 3], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0],
    // ╰ ╱ ╲ ╳ ╴ ╵ ╶ ╷
    [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 1], [1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0],
    // ╸ ╹ ╺ ╻ ╼ ╽ ╾ ╿
    [0, 0, 0, 2], [2, 0, 0, 0], [0, 2, 0, 0], [0, 0, 2, 0], [0, 2, 0, 1], [1, 0, 2, 0], [0, 1, 0, 2], [2, 0, 1, 0],
];

const UP: usize = 0;
const RIGHT: usize = 1;
const DOWN: usize = 2;
const LEFT: usize = 3;

/// The device-pixel shapes for `c` in a `w`×`h` cell.
fn pieces(c: char, w: i32, h: i32) -> Vec<Piece> {
    let light = (w as f32 / 8.0).round().max(1.0) as i32;
    let code = c as u32;
    match code {
        0x2504..=0x250B | 0x254C..=0x254F => {
            let (count, first) = match code {
                0x2504..=0x2507 => (3, 0x2504),
                0x2508..=0x250B => (4, 0x2508),
                _ => (2, 0x254C),
            };
            let weight = if (code - first) % 2 == 0 { 1 } else { 2 };
            dashes(count, weight, (code - first) >= 2, w, h, light)
        }
        0x256D..=0x2570 => {
            let (down, right) = match code {
                0x256D => (true, true),
                0x256E => (true, false),
                0x256F => (false, false),
                _ => (false, true),
            };
            vec![arc(down, right, w, h, light)]
        }
        0x2571..=0x2573 => {
            let rising = vec![[w as f32, 0.0], [0.0, h as f32]];
            let falling = vec![[0.0, 0.0], [w as f32, h as f32]];
            let width = light as f32;
            match code {
                0x2571 => vec![Piece::Line {
                    points: rising,
                    width,
                }],
                0x2572 => vec![Piece::Line {
                    points: falling,
                    width,
                }],
                _ => vec![
                    Piece::Line {
                        points: rising,
                        width,
                    },
                    Piece::Line {
                        points: falling,
                        width,
                    },
                ],
            }
        }
        0x2500..=0x257F => lines(BOX[(code - 0x2500) as usize], w, h, light),
        0x2580..=0x259F => blocks(code, w, h),
        0x2800..=0x28FF => braille(code as u8, w, h),
        0x23BE | 0x23BF => {
            // A light vertical with a light arm right, at the top or bottom.
            let x = span(1, w / 2, light);
            let corner = if code == 0x23BE {
                h / 4
            } else {
                h - h / 4 - light
            };
            let vertical = if code == 0x23BE {
                (corner, h)
            } else {
                (0, corner + light)
            };
            vec![fill(x, vertical), fill((x.0, w), (corner, corner + light))]
        }
        _ => Vec::new(),
    }
}

/// The lateral extent of a line of `weight` centred on `c`.
fn span(weight: u8, c: i32, light: i32) -> (i32, i32) {
    match weight {
        3 => (c - light - light / 2, c + light - light / 2 + light),
        _ => {
            let t = if weight == 2 { 2 * light } else { light };
            (c - t / 2, c - t / 2 + t)
        }
    }
}

/// A double line's two strokes, lower coordinate first.
fn double(c: i32, light: i32) -> [(i32, i32); 2] {
    let low = c - light - light / 2;
    let high = c + light - light / 2;
    [(low, low + light), (high, high + light)]
}

/// Lines from the cell's edge midpoints to its centre. Arms reach across the
/// centre to cover any perpendicular line. Where a double meets a double they
/// stop at its near stroke instead, leaving the gaps that make `╔ ╬ ╦` read.
fn lines(arms: [u8; 4], w: i32, h: i32, light: i32) -> Vec<Piece> {
    let mut out = Vec::new();
    for (direction, &weight) in arms.iter().enumerate() {
        if weight == 0 {
            continue;
        }
        let vertical = direction == UP || direction == DOWN;
        let from_start = direction == UP || direction == LEFT;
        let (length, axial, lateral) = if vertical {
            (h, h / 2, w / 2)
        } else {
            (w, w / 2, h / 2)
        };
        // Perpendicular arms on the lateral low and high sides.
        let (low_side, high_side) = if vertical { (LEFT, RIGHT) } else { (UP, DOWN) };
        let perpendicular_double = arms[low_side] == 3 || arms[high_side] == 3;
        let cover = [low_side, high_side]
            .into_iter()
            .filter(|&side| arms[side] != 0)
            .map(|side| span(arms[side], axial, light))
            .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
            .unwrap_or_else(|| span(weight.min(2), axial, light));
        let [near_low, near_high] = double(axial, light);
        let near = if from_start { near_low.1 } else { near_high.0 };
        let strokes: Vec<((i32, i32), bool)> = if weight == 3 {
            let [low, high] = double(lateral, light);
            vec![
                (low, perpendicular_double && arms[low_side] == 3),
                (high, perpendicular_double && arms[high_side] == 3),
            ]
        } else {
            let tee = arms[low_side] != 0 && arms[high_side] != 0 && arms[(direction + 2) % 4] == 0;
            vec![(span(weight, lateral, light), perpendicular_double && tee)]
        };
        for (across, stop_near) in strokes {
            let along = match (from_start, stop_near) {
                (true, true) => (0, near),
                (true, false) => (0, cover.1),
                (false, true) => (near, length),
                (false, false) => (cover.0, length),
            };
            out.push(if vertical {
                fill(across, along)
            } else {
                fill(along, across)
            });
        }
    }
    out
}

fn dashes(count: i32, weight: u8, vertical: bool, w: i32, h: i32, light: i32) -> Vec<Piece> {
    let (length, lateral) = if vertical { (h, w / 2) } else { (w, h / 2) };
    let across = span(weight, lateral, light);
    let gap = (length / count / 3).max(1);
    (0..count)
        .map(|i| {
            let along = (
                length * i / count + gap / 2,
                length * (i + 1) / count - (gap - gap / 2),
            );
            if vertical {
                fill(across, along)
            } else {
                fill(along, across)
            }
        })
        .collect()
}

/// A light quarter circle joining the centre lines of the two edges it names.
fn arc(down: bool, right: bool, w: i32, h: i32, light: i32) -> Piece {
    let x = span(1, w / 2, light);
    let y = span(1, h / 2, light);
    let (cx, cy) = ((x.0 + x.1) as f32 / 2.0, (y.0 + y.1) as f32 / 2.0);
    let (sx, sy) = (
        if right { 1.0 } else { -1.0 },
        if down { 1.0 } else { -1.0 },
    );
    let radius = (w as f32 / 2.0).min(h as f32 / 2.0);
    let (ox, oy) = (cx + sx * radius, cy + sy * radius);
    let mut points = vec![[cx, if down { h as f32 } else { 0.0 }]];
    const STEPS: usize = 12;
    for k in 0..=STEPS {
        let t = k as f32 / STEPS as f32 * std::f32::consts::FRAC_PI_2;
        points.push([ox - sx * radius * t.cos(), oy - sy * radius * t.sin()]);
    }
    points.push([if right { w as f32 } else { 0.0 }, cy]);
    Piece::Line {
        points,
        width: light as f32,
    }
}

/// U+2580–259F: halves, eighths, shades and quadrants.
fn blocks(code: u32, w: i32, h: i32) -> Vec<Piece> {
    let eighths = |length: i32, n: i32| (length * n + 4) / 8;
    let (mx, my) = (w / 2, h / 2);
    let shade = |alpha| {
        vec![Piece::Fill {
            x: (0, w),
            y: (0, h),
            alpha,
        }]
    };
    match code {
        0x2580 => vec![fill((0, w), (0, my))],
        0x2581..=0x2588 => vec![fill((0, w), (h - eighths(h, (code - 0x2580) as i32), h))],
        0x2589..=0x258F => vec![fill((0, eighths(w, (0x2590 - code) as i32)), (0, h))],
        0x2590 => vec![fill((mx, w), (0, h))],
        0x2591 => shade(64),
        0x2592 => shade(128),
        0x2593 => shade(192),
        0x2594 => vec![fill((0, w), (0, eighths(h, 1)))],
        0x2595 => vec![fill((w - eighths(w, 1), w), (0, h))],
        _ => {
            // Quadrant bits: 1 upper left, 2 upper right, 4 lower left, 8 lower right.
            let bits = [4, 8, 1, 13, 9, 7, 11, 2, 6, 14][(code - 0x2596) as usize];
            [
                (1, (0, mx), (0, my)),
                (2, (mx, w), (0, my)),
                (4, (0, mx), (my, h)),
                (8, (mx, w), (my, h)),
            ]
            .into_iter()
            .filter(|(bit, ..)| bits & bit != 0)
            .map(|(_, x, y)| fill(x, y))
            .collect()
        }
    }
}

/// U+2800–28FF: eight dots, 2 columns × 4 rows, one bit each.
fn braille(bits: u8, w: i32, h: i32) -> Vec<Piece> {
    // Dots 1–8 by bit: column, row.
    const DOTS: [(i32, i32); 8] = [
        (0, 0),
        (0, 1),
        (0, 2),
        (1, 0),
        (1, 1),
        (1, 2),
        (0, 3),
        (1, 3),
    ];
    let size = (w / 4).min(h / 8).max(1);
    DOTS.iter()
        .enumerate()
        .filter(|(bit, _)| bits & (1 << bit) != 0)
        .map(|(_, &(col, row))| {
            let x = (w * (2 * col + 1) + 2) / 4 - size / 2;
            let y = (h * (2 * row + 1) + 4) / 8 - size / 2;
            fill((x, x + size), (y, y + size))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `pieces` rasterised: one string per pixel row, `#` filled, `.` empty.
    fn raster(c: char, w: i32, h: i32) -> Vec<String> {
        let mut grid = vec![vec!['.'; w as usize]; h as usize];
        for piece in pieces(c, w, h) {
            if let Piece::Fill { x, y, .. } = piece {
                for row in grid.iter_mut().take(y.1 as usize).skip(y.0 as usize) {
                    for px in row.iter_mut().take(x.1 as usize).skip(x.0 as usize) {
                        *px = '#';
                    }
                }
            }
        }
        grid.into_iter()
            .map(|row| row.into_iter().collect())
            .collect()
    }

    #[test]
    fn covers_box_blocks_braille_and_gutter_corners_only() {
        for c in ['─', '╬', '╭', '▀', '▟', '⠀', '⣿', '⎿'] {
            assert_eq!(drawn(&c.to_string()), Some(c));
        }
        for text in ["a", "", "✻", "⏺", "─\u{301}", "──"] {
            assert_eq!(drawn(text), None, "{text:?}");
        }
    }

    #[test]
    fn box_arms_reach_exactly_the_edges_they_name() {
        // Every line character, at two densities: each named arm touches the
        // middle of its edge, so neighbouring cells join; others stay clear.
        for (w, h) in [(9, 18), (17, 36)] {
            for code in 0x2500..=0x257F_u32 {
                let arms = BOX[(code - 0x2500) as usize];
                if arms == [0; 4] {
                    continue;
                }
                let c = char::from_u32(code).unwrap();
                let grid = raster(c, w, h);
                let at = |x: i32, y: i32| grid[y as usize].as_bytes()[x as usize] == b'#';
                let edge = |direction: usize| {
                    let (x, y) = match direction {
                        UP => (w / 2, 0),
                        RIGHT => (w - 1, h / 2),
                        DOWN => (w / 2, h - 1),
                        _ => (0, h / 2),
                    };
                    // A double arm straddles the midpoint; look one stroke out.
                    let off = if arms[direction] == 3 {
                        (w as f32 / 8.0).round().max(1.0) as i32
                    } else {
                        0
                    };
                    if direction == UP || direction == DOWN {
                        at(x - off, y)
                    } else {
                        at(x, y - off)
                    }
                };
                for direction in 0..4 {
                    assert_eq!(
                        edge(direction),
                        arms[direction] != 0,
                        "{c} arm {direction} at {w}x{h}"
                    );
                }
            }
        }
    }

    #[test]
    fn doubles_leave_gaps_where_they_meet_and_corners_close() {
        let cross = raster('╬', 9, 18);
        assert_eq!(
            &cross[9][4..5],
            ".",
            "╬ centre is open:\n{}",
            cross.join("\n")
        );
        assert_eq!(raster('═', 9, 18)[9], ".........");
        let corner = raster('╔', 9, 18);
        assert_eq!(corner[8], "...######", "{}", corner.join("\n"));
        assert_eq!(corner[10], "...#.####", "{}", corner.join("\n"));
    }

    #[test]
    fn blocks_tile_and_braille_maps_bits_to_dots() {
        let (upper, lower) = (raster('▀', 9, 18), raster('▄', 9, 18));
        for (a, b) in upper.iter().zip(&lower) {
            assert!(
                a.chars()
                    .zip(b.chars())
                    .all(|(a, b)| (a == '#') != (b == '#'))
            );
        }
        assert!(raster('█', 9, 18).iter().all(|row| row == "#########"));
        assert!(pieces('⠀', 9, 18).is_empty());
        assert_eq!(pieces('⣿', 9, 18).len(), 8);
        // Dot 1 is top left, dot 8 bottom right.
        let one = raster('⠁', 16, 32);
        let eight = raster('⢀', 16, 32);
        assert!(one[4].starts_with("..####"), "{}", one.join("\n"));
        assert!(eight[28].ends_with("####.."), "{}", eight.join("\n"));
    }
}
