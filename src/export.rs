//! Pictures of the whole map with its objects, drawn on the CPU (the window only ever holds what's visible): drawn at
//! up to 32 px per tile, shrunk if asked, saved as PNG, JPEG or BMP.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use image::RgbaImage;
use image::imageops::{self, FilterType};
use op2_formats::Map;
use op2_formats::render::{self, MapTilesets};

use crate::scene::{self, Layers};
use crate::setup::{Kind, Setup};
use crate::sprites::{Art, Look};

/// The longest side a drawn map gets; a 512-wide map comes out at 16 px per tile.
const MAX_SIDE: u32 = 8192;

/// JPEG quality: small files for the web, with the map's tiles and units still sharp.
const JPEG_QUALITY: u8 = 85;

/// Pixels per tile [`draw`] uses for `map`: the game's 32, less for maps wider than 256 tiles.
pub fn scale_for(map: &Map) -> u32 {
    (MAX_SIDE / map.width.max(map.height)).clamp(1, 32)
}

/// Sets every tile's mapping index (bits 5..16) from `graphics`, row by row, as the mission's setup left the tiles;
/// ignored if it's the wrong size.
pub fn apply_graphics(map: &mut Map, graphics: &[u16]) {
    if graphics.len() != (map.width * map.height) as usize {
        return;
    }
    for y in 0..map.height {
        for x in 0..map.width {
            let bits = map.tile_bits(x, y);
            // 11 bits of mapping index, at bits 5..16 of the tile; the other bits stay as the map file has them.
            let mapping = graphics[(y * map.width + x) as usize] as u32 & 0x7FF;
            map.set_tile_bits(x, y, (bits & !(0x7FF << 5)) | mapping << 5);
        }
    }
}

/// The map at [`scale_for`] with what `layers` shows of `setup` on it: art where `art` has it, dots otherwise.
pub fn draw(map: &Map, tilesets: &MapTilesets, setup: Option<&Setup>, art: Option<&Art>, layers: &Layers) -> RgbaImage {
    let px = scale_for(map);
    let mut img = render::compose(map, tilesets, px);
    let (w, h) = (img.width as i32, img.height as i32);
    let scale = px as f32 / 32.0;
    // Composed sprites by look, so repeated units are composed once.
    let mut cache: HashMap<Look, Option<op2_formats::Sprite>> = HashMap::new();
    for (o, look) in setup.map(|s| scene::plan(s, layers)).unwrap_or_default() {
        let sprite = match (layers.art, art, look) {
            (true, Some(art), Some((look, at))) => {
                let s = cache.entry(look.clone()).or_insert_with(|| art.compose(&look));
                s.as_ref().map(|s| (s, at))
            }
            _ => None,
        };
        match sprite {
            Some((s, at)) => {
                // Nearest sampling: each destination pixel reads the sprite pixel it covers. `at` and the hotspot are
                // pixels at 32 px a tile, so they're scaled down to the image's `px`.
                let (sw, sh) = (s.image.width as i32, s.image.height as i32);
                let x0 = ((at.0 - s.hotspot.0 as f32) * scale).floor() as i32;
                let y0 = ((at.1 - s.hotspot.1 as f32) * scale).floor() as i32;
                let (dw, dh) = (((sw as f32) * scale).ceil() as i32, ((sh as f32) * scale).ceil() as i32);
                for dy in 0..dh {
                    for dx in 0..dw {
                        let (x, y) = (x0 + dx, y0 + dy);
                        if x < 0 || y < 0 || x >= w || y >= h {
                            continue;
                        }
                        let (sx, sy) = (((dx as f32 / scale) as i32).min(sw - 1), ((dy as f32 / scale) as i32).min(sh - 1));
                        let si = ((sy * sw + sx) * 4) as usize;
                        blend(&mut img.rgba, ((y * w + x) * 4) as usize, &s.image.rgba[si..si + 4]);
                    }
                }
            }
            None => {
                // A dot: a filled square, smaller for anything but a structure.
                let [r, g, b] = scene::dot_rgb(o);
                let size = ((px as f32) * if o.kind == Kind::Structure { 0.8 } else { 0.6 }).max(3.0) as i32;
                let (cx, cy) = (o.tile.0 * px as i32 + px as i32 / 2, o.tile.1 * px as i32 + px as i32 / 2);
                for y in cy - size / 2..cy - size / 2 + size {
                    for x in cx - size / 2..cx - size / 2 + size {
                        if (0..w).contains(&x) && (0..h).contains(&y) {
                            blend(&mut img.rgba, ((y * w + x) * 4) as usize, &[r, g, b, 255]);
                        }
                    }
                }
            }
        }
    }
    RgbaImage::from_raw(img.width, img.height, img.rgba).expect("composed image has its size")
}

/// `img` resized to `width` by `height` (Lanczos, which keeps small units and tile edges readable); as it is when
/// that's its size already.
pub fn resize(img: RgbaImage, width: u32, height: u32) -> RgbaImage {
    if (img.width(), img.height()) == (width, height) {
        return img;
    }
    imageops::resize(&img, width.max(1), height.max(1), FilterType::Lanczos3)
}

/// `img` shrunk so its longer side is at most `max_side`, keeping its shape; as it is when it's that small already.
pub fn fit(img: RgbaImage, max_side: u32) -> RgbaImage {
    let long = img.width().max(img.height());
    if long <= max_side {
        return img;
    }
    let f = max_side as f64 / long as f64;
    let (w, h) = ((img.width() as f64 * f).round() as u32, (img.height() as f64 * f).round() as u32);
    resize(img, w, h)
}

/// Saves `img` by `path`'s extension: `.jpg`/`.jpeg` as a JPEG, `.bmp` as a 24-bit BMP, anything else as a PNG.
pub fn save(path: &Path, img: RgbaImage) -> Result<()> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => {
            let rgb = image::DynamicImage::ImageRgba8(img).into_rgb8();
            let file = std::io::BufWriter::new(std::fs::File::create(path)?);
            image::codecs::jpeg::JpegEncoder::new_with_quality(file, JPEG_QUALITY).encode_image(&rgb)?;
        }
        "bmp" => image::DynamicImage::ImageRgba8(img).into_rgb8().save_with_format(path, image::ImageFormat::Bmp)?,
        _ => img.save_with_format(path, image::ImageFormat::Png)?,
    }
    Ok(())
}

/// [`draw`]s the map, shrinks it to at most `max_side` pixels on its longer side if given, and [`save`]s it.
pub fn save_map(path: &Path, map: &Map, tilesets: &MapTilesets, setup: Option<&Setup>, art: Option<&Art>, layers: &Layers, max_side: Option<u32>) -> Result<()> {
    let img = draw(map, tilesets, setup, art, layers);
    save(path, match max_side {
        Some(side) => fit(img, side),
        None => img,
    })
}

/// Straight-alpha "over" onto an opaque pixel.
fn blend(dst: &mut [u8], at: usize, src: &[u8]) {
    let a = src[3] as u32;
    for i in 0..3 {
        dst[at + i] = ((src[i] as u32 * a + dst[at + i] as u32 * (255 - a)) / 255) as u8;
    }
}
