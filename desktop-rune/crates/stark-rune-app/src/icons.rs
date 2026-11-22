//! Lucide icons from their SVG path data.
//!
//! Rune draws shapes from `Path` commands, while Lucide ships SVG `d`
//! strings. Rather than hand-converting every glyph (and losing arc fidelity),
//! this module parses the subset of the SVG path grammar Lucide uses: `M/m`,
//! `L/l`, `H/h`, `V/v`, `C/c`, `A/a` and `Z/z`, with elliptical arcs converted
//! to cubic Béziers. The 24-unit view box keeps the exact Lucide geometry.

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

/// Lucide `X`.
pub(crate) fn x(size: f32, color: rune::Color) -> Shape {
    lucide(&["M18 6 6 18", "m6 6 12 12"], size, 2.0, color)
}

/// Lucide `ArrowRight`.
pub(crate) fn arrow_right(size: f32, color: rune::Color) -> Shape {
    lucide(&["M5 12h14", "m12 5 7 7-7 7"], size, 2.0, color)
}

/// Lucide `ArrowLeft`.
pub(crate) fn arrow_left(size: f32, color: rune::Color) -> Shape {
    lucide(&["m12 19-7-7 7-7", "M19 12H5"], size, 2.0, color)
}

/// Lucide `LoaderCircle`: the open arc the reference UI spins while creating.
pub(crate) fn loader(size: f32, color: rune::Color) -> Shape {
    lucide(&["M21 12a9 9 0 1 1-6.219-8.56"], size, 2.0, color)
}

/// Lucide `FolderKanban` (group-project identity).
pub(crate) fn folder_kanban(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            "M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z",
            "M8 10v4",
            "M12 10v2",
            "M16 10v6",
        ],
        size,
        2.0,
        color,
    )
}

/// Lucide `PanelRightClose` (mobile details toggle while the rail is open).
pub(crate) fn panel_right_close(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            // rect 3,3 18×18 rx 2 (Lucide ships this as a `<rect>`).
            "M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2Z",
            "M15 3v18",
            "m8 9 3 3-3 3",
        ],
        size,
        2.0,
        color,
    )
}

/// Lucide `PanelRightOpen` (mobile details toggle while the chat shows).
pub(crate) fn panel_right_open(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            "M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2Z",
            "M15 3v18",
            "m10 15-3-3 3-3",
        ],
        size,
        2.0,
        color,
    )
}

/// Lucide `StickyNote` (group notes button and sheet).
pub(crate) fn sticky_note(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            "M21 9a2.4 2.4 0 0 0-.706-1.706l-3.588-3.588A2.4 2.4 0 0 0 15 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2z",
            "M15 3v5a1 1 0 0 0 1 1h5",
        ],
        size,
        2.0,
        color,
    )
}

/// Lucide `FolderOpen`.
pub(crate) fn folder_open(size: f32, color: rune::Color) -> Shape {
    lucide(
        &["m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2"],
        size,
        2.0,
        color,
    )
}

/// Lucide `Pause` (two 5×18 rounded bars).
pub(crate) fn pause(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            "M6 3h3a1 1 0 0 1 1 1v16a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1Z",
            "M15 3h3a1 1 0 0 1 1 1v16a1 1 0 0 1-1 1h-3a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1Z",
        ],
        size,
        2.0,
        color,
    )
}

/// Lucide `Play`.
pub(crate) fn play(size: f32, color: rune::Color) -> Shape {
    lucide(
        &["M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z"],
        size,
        2.0,
        color,
    )
}

/// Lucide `Volume2`.
pub(crate) fn volume(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            "M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z",
            "M16 9a5 5 0 0 1 0 6",
            "M19.364 18.364a9 9 0 0 0 0-12.728",
        ],
        size,
        2.0,
        color,
    )
}

/// Lucide `VolumeX`.
pub(crate) fn volume_x(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            "M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z",
            "m22 9-6 6",
            "m16 9 6 6",
        ],
        size,
        2.0,
        color,
    )
}

/// Lucide `MessageSquare` (the group chat kicker).
pub(crate) fn message_square(size: f32, color: rune::Color) -> Shape {
    lucide(
        &["M22 17a2 2 0 0 1-2 2H6.828a2 2 0 0 0-1.414.586l-2.202 2.202A.71.71 0 0 1 2 21.286V5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2z"],
        size,
        2.0,
        color,
    )
}

/// Lucide `TriangleAlert` (approvals, waiting chips, room notices).
pub(crate) fn triangle_alert(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            "m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3",
            "M12 9v4",
            "M12 17h.01",
        ],
        size,
        2.0,
        color,
    )
}

/// Lucide `Users`.
pub(crate) fn users(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2",
            "M16 3.128a4 4 0 0 1 0 7.744",
            "M22 21v-2a4 4 0 0 0-3-3.87",
            // circle cx 9 cy 7 r 4.
            "M5 7a4 4 0 1 0 8 0a4 4 0 1 0-8 0",
        ],
        size,
        2.0,
        color,
    )
}

/// Lucide `Sparkles` (empty group conversation).
pub(crate) fn sparkles(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            "M11.017 2.814a1 1 0 0 1 1.966 0l1.051 5.558a2 2 0 0 0 1.594 1.594l5.558 1.051a1 1 0 0 1 0 1.966l-5.558 1.051a2 2 0 0 0-1.594 1.594l-1.051 5.558a1 1 0 0 1-1.966 0l-1.051-5.558a2 2 0 0 0-1.594-1.594l-5.558-1.051a1 1 0 0 1 0-1.966l5.558-1.051a2 2 0 0 0 1.594-1.594z",
            "M20 2v4",
            "M22 4h-4",
            // circle cx 4 cy 20 r 2.
            "M2 20a2 2 0 1 0 4 0a2 2 0 1 0-4 0",
        ],
        size,
        2.0,
        color,
    )
}

/// Lucide `AtSign` (mention button and menu kicker).
pub(crate) fn at_sign(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            // circle cx 12 cy 12 r 4.
            "M8 12a4 4 0 1 0 8 0a4 4 0 1 0-8 0",
            "M16 8v5a3 3 0 0 0 6 0v-1a10 10 0 1 0-4 8",
        ],
        size,
        2.0,
        color,
    )
}

/// Lucide `Plus` (notes sheet's Add button).
pub(crate) fn plus(size: f32, color: rune::Color) -> Shape {
    lucide(&["M5 12h14", "M12 5v14"], size, 2.0, color)
}

/// Lucide `ChevronDown`.
pub(crate) fn chevron_down(size: f32, color: rune::Color) -> Shape {
    lucide(&["m6 9 6 6 6-6"], size, 2.0, color)
}

/// Lucide `ChevronRight`.
pub(crate) fn chevron_right(size: f32, color: rune::Color) -> Shape {
    lucide(&["m9 18 6-6-6-6"], size, 2.0, color)
}

/// Lucide `Clock`.
pub(crate) fn clock(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            // circle cx 12 cy 12 r 10.
            "M2 12a10 10 0 1 0 20 0a10 10 0 1 0-20 0",
            "M12 6v6l4 2",
        ],
        size,
        2.0,
        color,
    )
}

/// Lucide `Shield`.
pub(crate) fn shield(size: f32, color: rune::Color) -> Shape {
    lucide(
        &["M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"],
        size,
        2.0,
        color,
    )
}

/// Lucide `Bot` (the composer hint).
pub(crate) fn bot(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            "M12 8V4H8",
            // rect 4,8 16×12 rx 2.
            "M6 8h12a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-8a2 2 0 0 1 2-2Z",
            "M2 14h2",
            "M20 14h2",
            "M15 13v2",
            "M9 13v2",
        ],
        size,
        2.0,
        color,
    )
}

/// Lucide `Terminal` (the shell-enabled badge).
pub(crate) fn terminal(size: f32, color: rune::Color) -> Shape {
    lucide(&["M12 19h8", "m4 17 6-6-6-6"], size, 2.0, color)
}

/// Lucide `Info` (runtime notice).
pub(crate) fn info(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            // circle cx 12 cy 12 r 10.
            "M2 12a10 10 0 1 0 20 0a10 10 0 1 0-20 0",
            "M12 16v-4",
            "M12 8h.01",
        ],
        size,
        2.0,
        color,
    )
}

/// Lucide `CircleCheck` (queued-operation notice).
pub(crate) fn circle_check(size: f32, color: rune::Color) -> Shape {
    lucide(
        &[
            // circle cx 12 cy 12 r 10.
            "M2 12a10 10 0 1 0 20 0a10 10 0 1 0-20 0",
            "m9 12 2 2 4-4",
        ],
        size,
        2.0,
        color,
    )
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
            b'C' => loop {
                p.skip_ws();
                if !p.at_number() {
                    break;
                }
                let Some(mut c1) = p.point() else { break };
                let Some(mut c2) = p.point() else { break };
                let Some(mut to) = p.point() else { break };
                if rel {
                    c1[0] += cur[0];
                    c1[1] += cur[1];
                    c2[0] += cur[0];
                    c2[1] += cur[1];
                    to[0] += cur[0];
                    to[1] += cur[1];
                }
                path = path.cubic_to(c1, c2, to);
                cur = to;
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
    // SVG 2 F.6.5: the centre on the positive side is chosen when the
    // large-arc and sweep flags differ; `large == sweep` flips the sign.
    let sign = if large == sweep { -1.0 } else { 1.0 };
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

    #[test]
    fn group_icons_build_real_24_unit_geometry() {
        let color = rune::color::hex("#ffffff").unwrap();
        let icons: Vec<(&str, Shape)> = vec![
            ("folder-kanban", folder_kanban(18.0, color)),
            ("panel-right-close", panel_right_close(15.0, color)),
            ("panel-right-open", panel_right_open(15.0, color)),
            ("sticky-note", sticky_note(14.0, color)),
            ("folder-open", folder_open(15.0, color)),
            ("pause", pause(12.0, color)),
            ("play", play(12.0, color)),
            ("message-square", message_square(12.0, color)),
            ("triangle-alert", triangle_alert(12.0, color)),
            ("users", users(12.0, color)),
            ("sparkles", sparkles(20.0, color)),
            ("at-sign", at_sign(13.0, color)),
            ("plus", plus(12.0, color)),
            ("chevron-down", chevron_down(11.0, color)),
            ("chevron-right", chevron_right(12.0, color)),
            ("clock", clock(11.0, color)),
            ("shield", shield(12.0, color)),
            ("bot", bot(10.0, color)),
            ("terminal", terminal(9.0, color)),
            ("info", info(12.0, color)),
            ("circle-check", circle_check(12.0, color)),
        ];
        for (name, icon) in icons {
            assert!(!icon.is_empty(), "{name} drew nothing");
            // `view_box` is crate-private in Rune; Debug prints it, which keeps
            // this test honest without widening Rune's API.
            assert!(
                format!("{icon:?}").contains("view_box: Some([24.0, 24.0])"),
                "{name} must stay on the 24-unit Lucide box: {icon:?}"
            );
        }
    }
}
