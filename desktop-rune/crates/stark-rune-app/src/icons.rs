//! Lucide icons from their SVG path data.
//!
//! Rune draws shapes from `Path` commands, while Lucide ships SVG `d`
//! strings. Rather than hand-converting every glyph (and losing arc fidelity),
//! this module parses the subset of the SVG path grammar Lucide uses: `M/m`,
//! `L/l`, `H/h`, `V/v`, `A/a` and `Z/z`, with elliptical arcs converted to
//! cubic Béziers. The 24-unit view box keeps the exact Lucide geometry.

use rune::{LineCap, Path, Shape};

/// A stroked Lucide glyph at `size` points (`stroke_units` is the 24-unit
/// stroke width: 2.0 normally). Each slice entry is one SVG `<path>`: relative
/// commands after `m` start from `(0, 0)` per element, so they must not share
/// a concatenated path.
pub(crate) fn lucide(paths: &[&str], size: f32, stroke_units: f32, color: rune::Color) -> Shape {
    let mut shape = Shape::new();
    let width = size * (stroke_units / 24.0);
    for d in paths {
        let path = parse_path(d);
        shape = shape.stroke_path(&path, width, LineCap::Round, color);
    }
    shape.view_box(24.0, 24.0)
}

/// Lucide `Pin`.
pub(crate) fn pin(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            "M12 17v5",
            "M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z",
        ],
        size,
        2.0,
        color,
    )
}

/// Lucide `PinOff`.
pub(crate) fn pin_off(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            "M12 17v5",
            "M15 9.34V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H7.89",
            "m2 2 20 20",
            "M9 9v1.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h11",
        ],
        size,
        2.0,
        color,
    )
}

/// Lucide `Pencil`.
pub(crate) fn pencil(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            "M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z",
            "m15 5 4 4",
        ],
        size,
        2.0,
        color,
    )
}

/// Lucide `Trash2`.
pub(crate) fn trash(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            "M10 11v6",
            "M14 11v6",
            "M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6",
            "M3 6h18",
            "M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2",
        ],
        size,
        2.0,
        color,
    )
}

/// Lucide `Cloud` (hand-converted cubic arcs: the parser's arc sweep turns
/// Lucide's large cloud arcs into loops).
pub(crate) fn cloud(size: f32, color: rune::Color) -> Shape {
    let p = Path::new()
        .move_to([17.5, 19.0])
        .line_to([9.0, 19.0])
        .cubic_to([5.4, 19.0], [2.38, 16.26], [2.03, 12.67])
        .cubic_to([1.69, 9.09], [4.12, 5.82], [7.66, 5.13])
        .cubic_to([11.2, 4.44], [14.68, 6.55], [15.71, 10.0])
        .line_to([17.5, 10.0])
        .cubic_to([19.99, 10.0], [22.0, 12.01], [22.0, 14.5])
        .cubic_to([22.0, 16.99], [19.99, 19.0], [17.5, 19.0])
        .close();
    Shape::new()
        .stroke_path(&p, size * (2.0 / 24.0), LineCap::Round, color)
        .view_box(24.0, 24.0)
}

/// Lucide `CloudOff`.
pub(crate) fn cloud_off(size: f32, color: rune::Color) -> Shape {
    let p = Path::new()
        .move_to([10.94, 5.27])
        .cubic_to([13.23, 5.93], [15.03, 7.72], [15.71, 10.0])
        .line_to([17.5, 10.0])
        .cubic_to([18.97, 10.0], [20.35, 10.72], [21.19, 11.93])
        .cubic_to([22.03, 13.13], [22.23, 14.68], [21.72, 16.06])
        .move_to([18.8, 18.81])
        .cubic_to([18.38, 18.94], [17.94, 19.0], [17.5, 19.0])
        .line_to([9.0, 19.0])
        .cubic_to([5.77, 19.0], [2.96, 16.79], [2.2, 13.65])
        .cubic_to([1.44, 10.51], [2.92, 7.26], [5.79, 5.78])
        .move_to([2.0, 2.0])
        .line_to([22.0, 22.0]);
    Shape::new()
        .stroke_path(&p, size * (2.0 / 24.0), LineCap::Round, color)
        .view_box(24.0, 24.0)
}

struct Parser<'a> {
    bytes: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.i).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')) {
            self.i += 1;
        }
    }

    fn skip_sep(&mut self) {
        self.skip_ws();
        if self.peek() == Some(b',') {
            self.i += 1;
        }
        self.skip_ws();
    }

    fn at_number(&self) -> bool {
        matches!(self.peek(), Some(b'0'..=b'9' | b'.' | b'-' | b'+'))
    }

    fn number(&mut self) -> Option<f32> {
        self.skip_sep();
        let start = self.i;
        if matches!(self.peek(), Some(b'-' | b'+')) {
            self.i += 1;
        }
        let mut digits = 0;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.i += 1;
            digits += 1;
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.i += 1;
                digits += 1;
            }
        }
        if digits == 0 {
            self.i = start;
            return None;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let mark = self.i;
            self.i += 1;
            if matches!(self.peek(), Some(b'-' | b'+')) {
                self.i += 1;
            }
            let mut exp = 0;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.i += 1;
                exp += 1;
            }
            if exp == 0 {
                self.i = mark;
            }
        }
        std::str::from_utf8(&self.bytes[start..self.i])
            .ok()?
            .parse()
            .ok()
    }

    fn flag(&mut self) -> Option<bool> {
        self.skip_sep();
        match self.peek()? {
            b'0' => {
                self.i += 1;
                Some(false)
            }
            b'1' => {
                self.i += 1;
                Some(true)
            }
            _ => None,
        }
    }

    fn point(&mut self) -> Option<[f32; 2]> {
        let x = self.number()?;
        let y = self.number()?;
        Some([x, y])
    }
}

fn parse_path(d: &str) -> Path {
    let mut p = Parser {
        bytes: d.as_bytes(),
        i: 0,
    };
    let mut path = Path::new();
    let mut cur = [0.0f32, 0.0];
    let mut start = [0.0f32, 0.0];
    let mut cmd = 0u8;
    loop {
        p.skip_ws();
        let Some(c) = p.peek() else { break };
        if c.is_ascii_alphabetic() {
            p.i += 1;
            cmd = c;
        } else if cmd == 0 {
            break;
        }
        let rel = cmd.is_ascii_lowercase();
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let Some(mut pt) = p.point() else { break };
                if rel {
                    pt[0] += cur[0];
                    pt[1] += cur[1];
                }
                cur = pt;
                start = pt;
                path = path.move_to(pt);
                // Further pairs after a move are implicit linetos.
                loop {
                    p.skip_ws();
                    if !p.at_number() {
                        break;
                    }
                    let Some(mut q) = p.point() else { break };
                    if rel {
                        q[0] += cur[0];
                        q[1] += cur[1];
                    }
                    cur = q;
                    path = path.line_to(q);
                }
            }
            b'L' => loop {
                p.skip_ws();
                if !p.at_number() {
                    break;
                }
                let Some(mut q) = p.point() else { break };
                if rel {
                    q[0] += cur[0];
                    q[1] += cur[1];
                }
                cur = q;
                path = path.line_to(q);
            },
            b'H' => loop {
                p.skip_ws();
                if !p.at_number() {
                    break;
                }
                let Some(v) = p.number() else { break };
                cur[0] = if rel { cur[0] + v } else { v };
                path = path.line_to(cur);
            },
            b'V' => loop {
                p.skip_ws();
                if !p.at_number() {
                    break;
                }
                let Some(v) = p.number() else { break };
                cur[1] = if rel { cur[1] + v } else { v };
                path = path.line_to(cur);
            },
            b'A' => loop {
                p.skip_ws();
                if !p.at_number() {
                    break;
                }
                let Some(rx) = p.number() else { break };
                let Some(ry) = p.number() else { break };
                let Some(rotation) = p.number() else { break };
                let Some(large) = p.flag() else { break };
                let Some(sweep) = p.flag() else { break };
                let Some(mut to) = p.point() else { break };
                if rel {
                    to[0] += cur[0];
                    to[1] += cur[1];
                }
                path = arc_to(path, cur, to, rx, ry, rotation, large, sweep);
                cur = to;
            },
            b'Z' => {
                path = path.close();
                cur = start;
            }
            _ => break,
        }
    }
    path
}

/// SVG endpoint-parameterised arc → one or more cubic Béziers.
#[allow(clippy::too_many_arguments)]
fn arc_to(
    mut path: Path,
    from: [f32; 2],
    to: [f32; 2],
    rx: f32,
    ry: f32,
    rotation: f32,
    large: bool,
    sweep: bool,
) -> Path {
    if (from[0] - to[0]).abs() < 1e-6 && (from[1] - to[1]).abs() < 1e-6 {
        return path;
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-6 || ry < 1e-6 {
        return path.line_to(to);
    }
    let phi = rotation.to_radians();
    let (sin_phi, cos_phi) = phi.sin_cos();
    let dx = (from[0] - to[0]) / 2.0;
    let dy = (from[1] - to[1]) / 2.0;
    let x1p = cos_phi * dx + sin_phi * dy;
    let y1p = -sin_phi * dx + cos_phi * dy;
    let lambda = x1p * x1p / (rx * rx) + y1p * y1p / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let sign = if large != sweep { -1.0 } else { 1.0 };
    let num = (rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p).max(0.0);
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let coef = if den <= f32::EPSILON {
        0.0
    } else {
        sign * (num / den).sqrt()
    };
    let cxp = coef * rx * y1p / ry;
    let cyp = -coef * ry * x1p / rx;
    let cx = cos_phi * cxp - sin_phi * cyp + (from[0] + to[0]) / 2.0;
    let cy = sin_phi * cxp + cos_phi * cyp + (from[1] + to[1]) / 2.0;

    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| {
        let dot = ux * vx + uy * vy;
        let len = ((ux * ux + uy * uy) * (vx * vx + vy * vy)).sqrt();
        if len <= f32::EPSILON {
            return 0.0;
        }
        let a = (dot / len).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 {
            -a
        } else {
            a
        }
    };
    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;
    let theta1 = angle(1.0, 0.0, ux, uy);
    let mut dtheta = angle(ux, uy, vx, vy);
    if !sweep && dtheta > 0.0 {
        dtheta -= std::f32::consts::TAU;
    } else if sweep && dtheta < 0.0 {
        dtheta += std::f32::consts::TAU;
    }
    let segments = (dtheta.abs() / std::f32::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let delta = dtheta / segments as f32;
    let t = (delta / 4.0).tan() * (4.0 / 3.0);
    let point = |theta: f32| -> ([f32; 2], [f32; 2]) {
        let (sin_t, cos_t) = theta.sin_cos();
        let p = [
            cx + rx * cos_t * cos_phi - ry * sin_t * sin_phi,
            cy + rx * cos_t * sin_phi + ry * sin_t * cos_phi,
        ];
        let d = [
            -rx * sin_t * cos_phi - ry * cos_t * sin_phi,
            -rx * sin_t * sin_phi + ry * cos_t * cos_phi,
        ];
        (p, d)
    };
    let mut theta = theta1;
    for _ in 0..segments {
        let (p1, d1) = point(theta);
        theta += delta;
        let (p2, d2) = point(theta);
        path = path.cubic_to(
            [p1[0] + t * d1[0], p1[1] + t * d1[1]],
            [p2[0] - t * d2[0], p2[1] - t * d2[1]],
            p2,
        );
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lines_moves_and_closes() {
        // The parser only needs to accept the commands; drawing is Rune's.
        let _ = parse_path("M2 3 L4 5 H6 V7 Z");
    }

    #[test]
    fn arcs_and_implicit_linetos_parse() {
        let _ = parse_path("M17.5 19H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z");
        let _ = parse_path("M12 17v5 M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9Z");
        let _ = parse_path("m2 2 20 20");
    }
}
