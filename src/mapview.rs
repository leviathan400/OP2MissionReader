//! The map viewport. Every tile mapping's first frame goes into one texture atlas, and each frame draws the visible
//! tiles as a single mesh. Zoomed out past the point where tiles are a few pixels, a pre-composed overview image
//! (op2-formats' `render::compose`) is drawn instead, so the whole of a 512x256 map stays cheap.

use eframe::egui::{self, Color32, ColorImage, Mesh, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2, pos2, vec2};
use op2_formats::render::{self, MapTilesets};
use op2_formats::{Map, TILE_SIZE};

/// Tiles per atlas row: 64 x 32 px = 2048 px wide; a few thousand mappings stay well inside a 4096 px texture.
const ATLAS_COLUMNS: u32 = 64;
/// A tile's side in map pixels (32), as a float for screen maths.
const TILE: f32 = TILE_SIZE as f32;
/// Below this many screen pixels per tile the overview image is drawn instead of the tile mesh.
const MESH_MIN_TILE_PX: f32 = 6.0;
/// Screen pixels per map pixel, least and most.
const ZOOM_RANGE: (f32, f32) = (1.0 / 64.0, 8.0);
/// Shown for tiles whose mapping or tileset can't be resolved (as op2-formats' renderer does).
const MISSING: [u8; 4] = [255, 0, 255, 255];

/// The map, its textures and where it's looked at.
pub struct MapView {
    /// The map as shown: the file's tiles with the setup's graphics (walls, tubes, edits) applied.
    pub map: Map,
    /// The map as in the file.
    base: Map,
    /// The map's tilesets as loaded from the game folder (an entry is an error where one didn't load).
    pub tilesets: MapTilesets,
    /// Every mapping's first frame, one 32x32 cell each (see [`atlas`]).
    atlas: TextureHandle,
    /// The atlas's size in texels, for turning cells into UVs.
    atlas_size: Vec2,
    /// The whole map pre-composed, drawn when zoomed out below [`MESH_MIN_TILE_PX`]; rebuilt when the graphics change.
    overview: TextureHandle,
    /// Screen pixels per map pixel.
    pub zoom: f32,
    /// The map pixel at the viewport's top-left corner.
    pub origin: Vec2,
    /// Draw the tile grid (only where tiles are at least 12 screen pixels).
    pub grid: bool,
    /// File tiles to outline, top-left and bottom-right (the picked object's footprint).
    pub selected: Option<((i32, i32), (i32, i32))>,
    /// Fit the map to the viewport on the next frame (the viewport's size isn't known until then).
    fit_pending: bool,
    /// Centre this file tile on the next frame.
    center_pending: Option<(i32, i32)>,
    /// Where the map was last drawn, in screen points.
    pub rect: Rect,
}

/// What the viewport reports back each frame.
#[derive(Default)]
pub struct ViewState {
    /// The file tile under the mouse.
    pub hovered: Option<(u32, u32)>,
    /// The object the drawing callback found under the mouse.
    pub hovered_object: Option<u32>,
    /// Clicked (not dragged) this frame.
    pub clicked: bool,
}

/// Map pixels to screen points, for drawing on top of the map.
pub struct Transform {
    /// The viewport, in screen points.
    pub rect: Rect,
    /// The map pixel at the viewport's top-left corner.
    pub origin: Vec2,
    /// Screen pixels per map pixel.
    pub zoom: f32,
    /// The mouse, in screen points, when it's over the viewport.
    pub pointer: Option<egui::Pos2>,
}

impl Transform {
    /// The screen point of a map pixel.
    pub fn to_screen(&self, map_px: Vec2) -> egui::Pos2 {
        self.rect.min + (map_px - self.origin) * self.zoom
    }

    /// Screen pixels per tile.
    pub fn tile_px(&self) -> f32 {
        TILE * self.zoom
    }
}

impl MapView {
    /// Builds the atlas and the overview; the map is fitted to the viewport on the first frame.
    pub fn new(ctx: &egui::Context, map: Map, tilesets: MapTilesets) -> Self {
        let (atlas_image, atlas_size) = atlas(&map, &tilesets);
        let atlas = ctx.load_texture("map-atlas", atlas_image, TextureOptions::NEAREST);
        let overview = overview(ctx, &map, &tilesets);
        MapView {
            base: map.clone(),
            map,
            tilesets,
            atlas,
            atlas_size,
            overview,
            zoom: 1.0,
            origin: Vec2::ZERO,
            grid: false,
            selected: None,
            fit_pending: true,
            center_pending: None,
            rect: Rect::NOTHING,
        }
    }

    /// Shows the whole map on the next frame.
    pub fn fit(&mut self) {
        self.fit_pending = true;
    }

    /// Shows these tile graphics (mapping indices, row by row) instead of the file's; `None` shows the file's.
    pub fn set_graphics(&mut self, ctx: &egui::Context, graphics: Option<&[u16]>) {
        self.map = self.base.clone();
        if let Some(g) = graphics {
            crate::export::apply_graphics(&mut self.map, g);
        }
        self.overview = overview(ctx, &self.map, &self.tilesets);
    }

    /// Centres a file tile, zooming in to at least half size.
    pub fn center_on(&mut self, tile: (i32, i32)) {
        self.center_pending = Some(tile);
    }

    /// Centres a file tile at 100%.
    pub fn center_full_size(&mut self, tile: (i32, i32)) {
        self.fit_pending = false;
        self.zoom = 1.0;
        self.center_pending = Some(tile);
    }

    /// The map's size in map pixels.
    fn map_px(&self) -> Vec2 {
        vec2(self.map.width as f32, self.map.height as f32) * TILE
    }

    /// Draws the map, then `objects` on top of it (which returns the object under the mouse, if any).
    pub fn ui(&mut self, ui: &mut egui::Ui, objects: impl FnOnce(&egui::Painter, &Transform) -> Option<u32>) -> ViewState {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        self.rect = rect;
        if self.fit_pending {
            let fit = (rect.size() / self.map_px()).min_elem();
            self.zoom = fit.clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
            // Centred.
            self.origin = (self.map_px() - rect.size() / self.zoom) / 2.0;
            self.fit_pending = false;
        }
        if let Some((x, y)) = self.center_pending.take() {
            self.zoom = self.zoom.max(0.5);
            // The tile's centre (+0.5) at the viewport's centre.
            self.origin = (vec2(x as f32, y as f32) + Vec2::splat(0.5)) * TILE - rect.size() / self.zoom / 2.0;
        }
        if resp.dragged() {
            self.origin -= resp.drag_delta() / self.zoom;
        }
        if let Some(p) = resp.hover_pos() {
            // Wheel zooms around the cursor (pinch and Ctrl+wheel arrive as zoom_delta).
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta().y, i.zoom_delta()));
            let factor = pinch * (scroll * 0.0015).exp();
            if factor != 1.0 {
                // Keep the map pixel under the cursor where it is.
                let under = self.origin + (p - rect.min) / self.zoom;
                self.zoom = (self.zoom * factor).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
                self.origin = under - (p - rect.min) / self.zoom;
            }
        }

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_gray(20));
        let to_screen = |map_px: Vec2| rect.min + (map_px - self.origin) * self.zoom;
        let tile_px = TILE * self.zoom;
        if tile_px < MESH_MIN_TILE_PX {
            let whole = Rect::from_min_max(to_screen(Vec2::ZERO), to_screen(self.map_px()));
            painter.image(self.overview.id(), whole, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        } else {
            let (x0, x1, y0, y1) = self.visible_tiles(rect);
            let mut mesh = Mesh::with_texture(self.atlas.id());
            for y in y0..y1 {
                for x in x0..x1 {
                    let min = to_screen(vec2(x as f32, y as f32) * TILE);
                    let screen = Rect::from_min_size(min, Vec2::splat(tile_px));
                    mesh.add_rect_with_uv(screen, self.uv(self.map.tile(x, y).mapping_index), Color32::WHITE);
                }
            }
            painter.add(mesh);
            if self.grid && tile_px >= 12.0 {
                let stroke = Stroke::new(1.0, Color32::from_black_alpha(90));
                let (top, bottom) = (to_screen(vec2(0.0, y0 as f32 * TILE)).y, to_screen(vec2(0.0, y1 as f32 * TILE)).y);
                let (left, right) = (to_screen(vec2(x0 as f32 * TILE, 0.0)).x, to_screen(vec2(x1 as f32 * TILE, 0.0)).x);
                for x in x0..=x1 {
                    let sx = to_screen(vec2(x as f32 * TILE, 0.0)).x;
                    painter.line_segment([pos2(sx, top), pos2(sx, bottom)], stroke);
                }
                for y in y0..=y1 {
                    let sy = to_screen(vec2(0.0, y as f32 * TILE)).y;
                    painter.line_segment([pos2(left, sy), pos2(right, sy)], stroke);
                }
            }
        }
        let hovered_object = objects(&painter, &Transform { rect, origin: self.origin, zoom: self.zoom, pointer: resp.hover_pos() });

        // The file tile under the mouse, if it's over the map and not the dark margin around it.
        let hovered = resp.hover_pos().and_then(|p| {
            let m = (self.origin + (p - rect.min) / self.zoom) / TILE;
            (m.x >= 0.0 && m.y >= 0.0 && (m.x as u32) < self.map.width && (m.y as u32) < self.map.height)
                .then_some((m.x as u32, m.y as u32))
        });
        if let Some(((x1, y1), (x2, y2))) = self.selected {
            let min = to_screen(vec2(x1 as f32, y1 as f32) * TILE);
            let max = to_screen(vec2((x2 + 1) as f32, (y2 + 1) as f32) * TILE);
            // At least 12 px across, so it stays visible zoomed out.
            let r = Rect::from_two_pos(min, max);
            let r = Rect::from_center_size(r.center(), r.size().max(Vec2::splat(12.0)));
            painter.rect_stroke(r.expand(2.0), 2.0, Stroke::new(3.0, Color32::BLACK), egui::StrokeKind::Inside);
            painter.rect_stroke(r.expand(2.0), 2.0, Stroke::new(1.5, Color32::from_rgb(80, 220, 255)), egui::StrokeKind::Inside);
        }
        if let Some((x, y)) = hovered
            && tile_px >= 8.0
        {
            let min = to_screen(vec2(x as f32, y as f32) * TILE);
            painter.rect_stroke(Rect::from_min_size(min, Vec2::splat(tile_px)), 0.0, Stroke::new(1.5, Color32::YELLOW), egui::StrokeKind::Inside);
        }
        ViewState { hovered, hovered_object, clicked: resp.clicked() }
    }

    /// The tile columns and rows (half-open ranges) that overlap the viewport.
    fn visible_tiles(&self, rect: Rect) -> (u32, u32, u32, u32) {
        let lo = self.origin / TILE;
        let hi = (self.origin + rect.size() / self.zoom) / TILE;
        let clamp = |v: f32, max: u32| (v.max(0.0) as u32).min(max);
        (clamp(lo.x.floor(), self.map.width), clamp(hi.x.ceil(), self.map.width), clamp(lo.y.floor(), self.map.height), clamp(hi.y.ceil(), self.map.height))
    }

    /// The atlas cell of a tile mapping, inset a quarter texel so nearest sampling never reads the neighbour.
    fn uv(&self, mapping: u16) -> Rect {
        // Indices past the end use the extra MISSING cell after the last mapping.
        let mapping = (mapping as usize).min(self.map.mappings.len());
        let (col, row) = (mapping as u32 % ATLAS_COLUMNS, mapping as u32 / ATLAS_COLUMNS);
        let min = vec2(col as f32, row as f32) * TILE + Vec2::splat(0.25);
        let max = min + Vec2::splat(TILE - 0.5);
        Rect::from_min_max((min / self.atlas_size).to_pos2(), (max / self.atlas_size).to_pos2())
    }
}

/// The whole map in one texture, as big as the GPU allows up to 4096 px on the long side (some drivers stop at 2048:
/// 4 px per tile for a 512-wide map then).
fn overview(ctx: &egui::Context, map: &Map, tilesets: &MapTilesets) -> TextureHandle {
    let max_side = ctx.input(|i| i.max_texture_side).min(4096) as u32;
    // Map pixels per tile in the image: whole pixels, 1 to the full 32.
    let px = (max_side / map.width.max(map.height)).clamp(1, TILE_SIZE);
    let img = render::compose(map, tilesets, px);
    ctx.load_texture(
        "map-overview",
        ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.rgba),
        TextureOptions::LINEAR,
    )
}

/// The first frame of every tile mapping in `map`, in a grid of [`ATLAS_COLUMNS`] cells, plus one more cell after
/// them for indices past the end. Mappings whose tileset didn't load show as [`MISSING`], as does that extra cell.
fn atlas(map: &Map, tilesets: &MapTilesets) -> (ColorImage, Vec2) {
    let count = map.mappings.len() as u32 + 1;
    let (w, h) = (ATLAS_COLUMNS * TILE_SIZE, count.div_ceil(ATLAS_COLUMNS) * TILE_SIZE);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let row_bytes = (TILE_SIZE * 4) as usize;
    for i in 0..count {
        let (col, row) = (i % ATLAS_COLUMNS, i / ATLAS_COLUMNS);
        let pixels = map.mappings.get(i as usize).and_then(|m| tilesets.get(m.tileset)?.tile(m.graphic as u32));
        for y in 0..TILE_SIZE {
            let at = (((row * TILE_SIZE + y) * w + col * TILE_SIZE) * 4) as usize;
            let dst = &mut rgba[at..at + row_bytes];
            match pixels {
                Some(p) => p.write_row(y, dst),
                None => dst.as_chunks_mut::<4>().0.fill(MISSING),
            }
        }
    }
    (ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba), vec2(w as f32, h as f32))
}
