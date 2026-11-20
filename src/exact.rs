//! A whole-map image drawn the way the game draws its detail view, pixel for pixel: from an OP2TestHarness state
//! (`state_<tick>.json` with `views`), this draws the same pixels as the harness's image of it (`map_<tick>.png`).
//! OP2Imager's state drawing uses it.
//!
//! - **Colour.** The game draws into a 15-bit surface (5 bits a channel; GDI widens each to 8 bits by repeating its top
//!   bits), so terrain and a sprite's palette colours land there truncated to 5 bits, and a shadow pixel darkens what
//!   is under it to `(c >> 1) + (c >> 2)` of each 5-bit channel.
//! - **Sprites.** A frame's layers are drawn in file order through the viewport's frame draw (0x403F20), at the
//!   object's pixel less the animation's displacement plus the layer's offset. A layer whose image type holds a
//!   number `k` > 0 in bits 5-9 is drawn only inside the animation's appendix rectangle `k` (counted from 1).
//! - **Lines.** Lasers go through the surface's line draw (0x41EA10): clipped to the view, then two pixels thick, the
//!   colour ORed in. Microwaves are dashed lines (0x41EE00), stepped the same way.
//! - **Pieces.** The harness's image is its view pieces, each drawn by the game and copied whole over the ones before,
//!   so a sprite a piece culls at its edge stays cut off there: [`compose`] builds it the same way, and
//!   [`Canvas::shrink`] averages it down as the harness does.

use op2_formats::Map;
use op2_formats::art::Tint;
use op2_formats::render::MapTilesets;
use op2_formats::Sprites;

/// The game's animation numbers are op2_art.prt's index plus this.
pub const ANIM_BASE: i32 = 101;

/// One sprite draw: an animation (the game's number) and frame at an object's map pixel (file coordinates, no
/// padding), coloured for a player colour (`None`: the palette as stored).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Draw {
    /// The game's animation number (op2_art.prt's index plus [`ANIM_BASE`]).
    pub anim: i32,
    /// Frame of that animation, from 0.
    pub frame: i32,
    /// Map pixel x, file coordinates.
    pub x: i32,
    /// Map pixel y.
    pub y: i32,
    /// The player colour index whose palette ramp the sprite is drawn with.
    pub color: Option<u8>,
}

/// A line as the surface's line draws draw it: from (x1, y1) to (x2, y2) in map pixels, clipped to the view it was
/// drawn in (`clip`: left, top, right, bottom, map pixels). A laser's (0x41EA10) ORs its colour's low 15 bits into
/// each pixel; a dashed one's (0x41EE00, `dash`: the pattern and how far it's rotated first) writes its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Line {
    /// The first end's map pixel x (file coordinates); the walk starts here.
    pub x1: i32,
    pub y1: i32,
    /// The second end's map pixel x.
    pub x2: i32,
    pub y2: i32,
    /// The view's clip rectangle: left, top, right, bottom in map pixels.
    pub clip: [i32; 4],
    /// The colour as the game passes it: a 15-bit colour in each half (a laser's halves are the same).
    pub color: u32,
    /// A dashed line's 32-bit pattern and how many bits it's rotated left before the first step; `None` for a laser.
    pub dash: Option<(u32, u32)>,
}

/// One thing the detail view draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Item {
    /// A sprite: one frame of an animation.
    Frame(Draw),
    /// A laser or microwave line.
    Line(Line),
}

/// Part of a map drawn at 32 px a tile: RGB, row by row, its top-left at map pixel (`x0`, `y0`).
#[derive(Clone)]
pub struct Canvas {
    /// Map pixel x of the canvas's left column (file coordinates).
    pub x0: i32,
    /// Map pixel y of its top row.
    pub y0: i32,
    /// In pixels.
    pub width: u32,
    /// In pixels.
    pub height: u32,
    /// 3 bytes a pixel, each channel a 5-bit value widened to 8 bits (`widen5`).
    pub rgb: Vec<u8>,
}

/// A channel as the game's 15-bit surface keeps it, widened back to 8 bits as GDI does.
fn widen5(c5: u8) -> u8 {
    (c5 << 3) | (c5 >> 2)
}

impl Canvas {
    /// The map's tiles at 32 px a tile in the surface's 15-bit colours, each animated tile at the frame the game shows
    /// for animation counter `counter` (0x56EB1C, a state's `animCounter`; the tick in every state measured):
    /// `(counter / (delay + 1)) % (count + 1)` frames on from its first (measured on a vent tile of 11 frames and
    /// delay 10 at ticks 5, 5,000 and 10,000). A tile that can't be resolved is magenta, as in `render::compose`.
    pub fn terrain(map: &Map, tilesets: &MapTilesets, counter: u32) -> Canvas {
        let (width, height) = (map.width * 32, map.height * 32);
        let mut rgb = vec![0u8; (width * height * 3) as usize];
        for ty in 0..map.height {
            for tx in 0..map.width {
                let pixels = map.mapping(map.tile(tx, ty)).and_then(|m| {
                    let frame = if m.animation_count > 0 {
                        (counter / (m.animation_delay as u32 + 1)) % (m.animation_count as u32 + 1)
                    } else {
                        0
                    };
                    tilesets.get(m.tileset)?.tile(m.graphic as u32 + frame)
                });
                for y in 0..32 {
                    for x in 0..32 {
                        let c = pixels.as_ref().map_or([255, 0, 255, 255], |t| t.pixel(x, y));
                        let at = (((ty * 32 + y) * width + tx * 32 + x) * 3) as usize;
                        for i in 0..3 {
                            rgb[at + i] = widen5(c[i] >> 3);
                        }
                    }
                }
            }
        }
        Canvas { x0: 0, y0: 0, width, height, rgb }
    }

    /// The part of this canvas from map pixel (`x`, `y`), `width` by `height`, cut to what it holds.
    pub fn crop(&self, x: i32, y: i32, width: i32, height: i32) -> Canvas {
        let (l, t) = (x.max(self.x0), y.max(self.y0));
        let (r, b) = ((x + width).min(self.x0 + self.width as i32), (y + height).min(self.y0 + self.height as i32));
        let (w, h) = ((r - l).max(0) as usize, (b - t).max(0) as usize);
        let mut rgb = Vec::with_capacity(w * h * 3);
        for row in 0..h {
            let at = (((t - self.y0) as usize + row) * self.width as usize + (l - self.x0) as usize) * 3;
            rgb.extend_from_slice(&self.rgb[at..at + w * 3]);
        }
        Canvas { x0: l, y0: t, width: w as u32, height: h as u32, rgb }
    }

    /// `part` copied over this canvas where they overlap.
    pub fn paste(&mut self, part: &Canvas) {
        let w = part.width as usize;
        for row in 0..part.height as usize {
            let (x, y) = (part.x0 - self.x0, part.y0 - self.y0 + row as i32);
            if y < 0 || y >= self.height as i32 || x < 0 || x + w as i32 > self.width as i32 {
                continue;
            }
            let at = (y as usize * self.width as usize + x as usize) * 3;
            self.rgb[at..at + w * 3].copy_from_slice(&part.rgb[row * w * 3..(row + 1) * w * 3]);
        }
    }

    /// The index of map pixel (`x`, `y`) in `rgb`, if this canvas holds it.
    fn at(&self, x: i32, y: i32) -> Option<usize> {
        let (x, y) = (x - self.x0, y - self.y0);
        (x >= 0 && y >= 0 && x < self.width as i32 && y < self.height as i32).then(|| (y as usize * self.width as usize + x as usize) * 3)
    }

    /// Draws `item` as the game does.
    pub fn paint(&mut self, sprites: &Sprites, item: &Item) {
        match item {
            Item::Frame(d) => self.draw(sprites, d),
            Item::Line(l) => self.line(l),
        }
    }

    /// Draws a line as the surface's line draws do: the ends clipped to `clip` less one pixel on the right and
    /// bottom (the line is two pixels thick; 0x41E560 clips), then a Bresenham walk from the first end that stops on
    /// the second, each step's pixel and the ones right, below and right-below it drawn. A laser ORs its colour in. A
    /// dashed line draws a step only where its pattern (rotated left first, then one bit a step) has bit 0 set, writing
    /// its colour as a 32-bit pair (the low half left, the high half right), and takes 3 off each half after every
    /// drawn step, back to the full colour at each gap.
    pub fn line(&mut self, l: &Line) {
        let [left, top, right, bottom] = l.clip;
        let Some((x1, y1, x2, y2)) = clip_line((l.x1, l.y1, l.x2, l.y2), (left, top, right - 1, bottom - 1)) else { return };
        let (dx, dy) = (x2 - x1, y2 - y1);
        let (dx2, dy2) = (2 * dx.abs(), 2 * dy.abs());
        let (sx, sy) = (dx.signum(), dy.signum());
        // A 15-bit colour (5 bits each, red highest) split into its channels.
        let five = |c: u32| [(c >> 10 & 31) as u8, (c >> 5 & 31) as u8, (c & 31) as u8];
        let mut put = |px: i32, py: i32, c: [u8; 3], or: bool| {
            let Some(at) = self.at(px, py) else { return };
            for (dst, c5) in self.rgb[at..at + 3].iter_mut().zip(c) {
                *dst = widen5(if or { (*dst >> 3) | c5 } else { c5 });
            }
        };
        let mut pattern = l.dash.map(|(p, r)| p.rotate_left(r & 31));
        let mut color = l.color;
        let mut plot = |x: i32, y: i32| match &mut pattern {
            None => {
                let c = five(l.color);
                for (px, py) in [(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)] {
                    put(px, py, c, true);
                }
            }
            Some(p) => {
                if *p & 1 != 0 {
                    let (left, right) = (five(color), five(color >> 16));
                    for py in [y, y + 1] {
                        put(x, py, left, false);
                        put(x + 1, py, right, false);
                    }
                    // 3 off each 16-bit half at once.
                    color = color.wrapping_sub(0x30003);
                } else {
                    color = l.color;
                }
                *p = p.rotate_left(1);
            }
        };
        let (mut x, mut y) = (x1, y1);
        // the major axis steps every time, the minor one when the error isn't negative
        let x_major = dy2 < dx2;
        let (major, minor) = if x_major { (dx2, dy2) } else { (dy2, dx2) };
        let mut err = minor - (major >> 1);
        loop {
            plot(x, y);
            if (x, y) == (x2, y2) {
                break;
            }
            let both = err >= 0;
            if x_major || both {
                x += sx;
            }
            if !x_major || both {
                y += sy;
            }
            err += if both { minor - major } else { minor };
        }
    }

    /// Draws one frame as the game's frame draw does: each layer at the frame's position, a player-coloured palette
    /// when `d.color` is set, transparent index 0 skipped, a shadow layer darkening what is under it, a layer with an
    /// appendix rectangle clipped to it. Draws of missing animations or frames are skipped.
    pub fn draw(&mut self, sprites: &Sprites, d: &Draw) {
        let Some(a) = usize::try_from(d.anim - ANIM_BASE).ok().and_then(|i| sprites.animation(i)) else { return };
        let Some(f) = usize::try_from(d.frame).ok().and_then(|i| a.frames.get(i)) else { return };
        let tint = d.color.map_or(Tint::Stored, Tint::Player);
        let (ox, oy) = (d.x - a.displacement.0, d.y - a.displacement.1);
        for layer in &f.layers {
            let meta = sprites.art.images[layer.image as usize];
            let Ok(indices) = sprites.image_indices(layer.image as usize) else { continue };
            let palette = sprites.palette(&meta, tint);
            // Bits 5-9 of the image type: the appendix rectangle the layer is clipped to (1-based; 0 for none).
            let k = ((meta.kind & 0x3E0) >> 5) as usize;
            let clip = k.checked_sub(1).and_then(|k| a.appendix.get(k)).map(|r| r.map(|v| v as i32));
            let shadow = meta.is_shadow();
            for (i, &index) in indices.iter().enumerate() {
                if index == 0 {
                    continue;
                }
                let (lx, ly) = (layer.x as i32 + (i as u32 % meta.width) as i32, layer.y as i32 + (i as u32 / meta.width) as i32);
                if let Some(r) = clip
                    && (lx < r[0] || lx > r[2] || ly < r[1] || ly > r[3])
                {
                    continue;
                }
                let Some(at) = self.at(ox + lx, oy + ly) else { continue };
                let px = &mut self.rgb[at..at + 3];
                if shadow {
                    for c in px.iter_mut() {
                        let c5 = *c >> 3;
                        *c = widen5((c5 >> 1) + (c5 >> 2));
                    }
                } else {
                    let rgb = palette[index as usize];
                    for (c, s) in px.iter_mut().zip(rgb) {
                        *c = widen5(s >> 3);
                    }
                }
            }
        }
    }

    /// The top-left `width` by `height` pixels, shrunk `32 / px` times by averaging each block (sums truncated), as
    /// the harness shrinks its whole-map images.
    pub fn shrink(&self, width: u32, height: u32, px: u32) -> (u32, u32, Vec<u8>) {
        let f = (32 / px.clamp(1, 32)) as usize;
        let (w, h) = (width.min(self.width) as usize / f, height.min(self.height) as usize / f);
        let mut out = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                for c in 0..3 {
                    let mut sum = 0u32;
                    for dy in 0..f {
                        for dx in 0..f {
                            sum += self.rgb[((y * f + dy) * self.width as usize + x * f + dx) * 3 + c] as u32;
                        }
                    }
                    out[(y * w + x) * 3 + c] = (sum / (f * f) as u32) as u8;
                }
            }
        }
        (w as u32, h as u32, out)
    }
}

/// One view piece of a whole-map image as OP2TestHarness's state lists it (`views`): the view's top-left in game
/// pixels (padding included) and its draws in the order the game made them: a sprite as (animation, frame, x, y,
/// colour, light column), a line as (-1, colour, x1, y1, x2, y2), a dashed one as (-2, colour, x1, y1, x2, y2,
/// pattern, rotation), positions within the view.
pub struct View {
    /// The view's left edge in game pixels (padding included).
    pub left: i32,
    /// The view's top edge in pixels.
    pub top: i32,
    /// Each draw's numbers as listed, in the forms above.
    pub draws: Vec<Vec<i32>>,
    /// The surface's clip rectangle (left, top, right, bottom, within the view), listed when the view drew a line.
    pub clip: Option<[i32; 4]>,
}

/// The views of a state's `views`; none when it has none.
pub fn views_of(state: &serde_json::Value) -> Vec<View> {
    let num = |v: &serde_json::Value| v.as_i64().unwrap_or(0) as i32;
    state["views"].as_array().map(|vs| vs.iter().map(|v| View {
        left: num(&v["left"]),
        top: num(&v["top"]),
        draws: v["draws"].as_array().map(|ds| ds.iter().filter_map(|d| Some(d.as_array()?.iter().map(num).collect())).collect())
            .unwrap_or_default(),
        clip: v["clip"].as_array().filter(|c| c.len() == 4).map(|c| std::array::from_fn(|i| num(&c[i]))),
    }).collect()).unwrap_or_default()
}

/// A view's draws in map coordinates (file coordinates, the padding taken off), in its order. `colors[n]` is player
/// `n`'s colour (a state's `players[n].colour`); `padding` the map's padding in tiles. Entries too short to be a draw
/// are skipped.
pub fn items(v: &View, padding: i32, colors: &[u8]) -> Vec<Item> {
    let (ox, oy) = (v.left - padding * 32, v.top);
    v.draws.iter().filter(|n| n.len() >= 6).map(|n| {
        if n[0] == -1 || (n[0] == -2 && n.len() >= 8) {
            // a line, (-1, colour, x1, y1, x2, y2), or a dashed one with its pattern and rotation; clipped to the
            // view's surface
            let c = v.clip.unwrap_or([0, 0, i32::MAX / 4, i32::MAX / 4]);
            Item::Line(Line {
                x1: ox + n[2], y1: oy + n[3], x2: ox + n[4], y2: oy + n[5], color: n[1] as u32,
                clip: [ox + c[0], oy + c[1], ox + c[2], oy + c[3]],
                dash: (n[0] == -2).then(|| (n[6] as u32, n[7] as u32)),
            })
        } else {
            // the owner in the low 4 bits (a structure's draw has flags above them); -1 the palette as stored
            let color = if n[4] == -1 { None } else { colors.get((n[4] & 0xF) as usize).copied() };
            Item::Frame(Draw { anim: n[0], frame: n[1], x: ox + n[2], y: oy + n[3], color })
        }
    }).collect()
}

/// The whole-map image as the harness makes it, at 32 px a tile: each view, `view_size` pixels from its top-left (a
/// state's `viewSize`), drawn over `terrain` ([`Canvas::terrain`]) with its own draws and copied over the ones before
/// it. A sprite a view culls stays cut off at its edge. `padding` and `colors` as in [`items`].
pub fn compose(terrain: &Canvas, sprites: &Sprites, views: &[View], view_size: (i32, i32), padding: i32, colors: &[u8]) -> Canvas {
    let mut out = terrain.clone();
    for v in views {
        let mut part = terrain.crop(v.left - padding * 32, v.top, view_size.0, view_size.1);
        for item in items(v, padding, colors) {
            part.paint(sprites, &item);
        }
        out.paste(&part);
    }
    out
}

/// The surface's line clip (0x41E560), Cohen-Sutherland: while an end is outside `(left, top, right, bottom)`
/// (edges inside), move it onto the first edge it is beyond, in the order left, right, top, bottom, the other
/// coordinate found by integer division (truncating); `None` when both ends are beyond one edge.
fn clip_line(mut p: (i32, i32, i32, i32), r: (i32, i32, i32, i32)) -> Option<(i32, i32, i32, i32)> {
    // Outcodes: 1 left of the rectangle, 4 right of it, 2 above, 8 below.
    let code = |x: i32, y: i32| {
        let mut c = if x < r.0 { 1 } else if x > r.2 { 4 } else { 0 };
        c |= if y < r.1 { 2 } else if y > r.3 { 8 } else { 0 };
        c
    };
    loop {
        let (mut c1, c2) = (code(p.0, p.1), code(p.2, p.3));
        if c1 & c2 != 0 {
            return None;
        }
        if c1 | c2 == 0 {
            return Some(p);
        }
        // The first end is inside: swap the ends and move the second one; they stay swapped in the result.
        if c1 == 0 {
            p = (p.2, p.3, p.0, p.1);
            c1 = c2;
        }
        let (x1, y1, x2, y2) = p;
        if c1 & 1 != 0 {
            p.1 = y2 + (x2 - r.0) * (y1 - y2) / (x2 - x1);
            p.0 = r.0;
        } else if c1 & 4 != 0 {
            p.1 = y2 + (r.2 - x2) * (y1 - y2) / (x1 - x2);
            p.0 = r.2;
        } else if c1 & 2 != 0 {
            p.0 = x2 + (y2 - r.1) * (x1 - x2) / (y2 - y1);
            p.1 = r.1;
        } else {
            p.0 = x2 + (r.3 - y2) * (x1 - x2) / (y1 - y2);
            p.1 = r.3;
        }
    }
}
