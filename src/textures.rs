//! The window's object art: each distinct look becomes one texture, made the first time it's drawn.

use std::collections::HashMap;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions, Vec2, vec2};

use crate::sprites::{Art, Look};

/// A composed look, uploaded as a texture.
pub struct Drawn {
    /// The image, sampled nearest-neighbour so zoomed art stays sharp.
    pub texture: TextureHandle,
    /// The pixel of the image that sits on the object's reference pixel.
    pub hotspot: Vec2,
    /// The image's width and height in pixels.
    pub size: Vec2,
}

/// The game's art and the textures made from it so far.
pub struct ArtTextures {
    /// The art the textures are composed from.
    pub art: Art,
    /// `None` for looks without art, so they aren't composed again every frame.
    cache: HashMap<Look, Option<Drawn>>,
}

impl ArtTextures {
    /// Wraps `art` with no textures made yet.
    pub fn new(art: Art) -> ArtTextures {
        ArtTextures { art, cache: HashMap::new() }
    }

    /// The texture for `look`, composed the first time it's asked for; `None` when it has no art.
    pub fn get(&mut self, ctx: &egui::Context, look: &Look) -> Option<&Drawn> {
        if !self.cache.contains_key(look) {
            let drawn = self.art.compose(look).map(|s| {
                let size = [s.image.width as usize, s.image.height as usize];
                let texture = ctx.load_texture(format!("{look:?}"), ColorImage::from_rgba_unmultiplied(size, &s.image.rgba), TextureOptions::NEAREST);
                Drawn { texture, hotspot: vec2(s.hotspot.0 as f32, s.hotspot.1 as f32), size: vec2(size[0] as f32, size[1] as f32) }
            });
            self.cache.insert(look.clone(), drawn);
        }
        self.cache[look].as_ref()
    }
}
