//! What a picture of the map shows and in which order: the layers, each object's art and anchor, and the colours of
//! objects drawn as dots.

use std::collections::BTreeSet;

use crate::setup::{Kind, Object, Setup};
use crate::sprites::Look;

/// What's shown on the map.
#[derive(Debug, Clone)]
pub struct Layers {
    /// Any objects at all; off hides every kind regardless of the switches below.
    pub objects: bool,
    /// Art rather than dots.
    pub art: bool,
    /// Objects of `Kind::Vehicle`.
    pub vehicles: bool,
    /// Objects of `Kind::Structure`.
    pub structures: bool,
    /// Mining beacons, magma vents, fumaroles.
    pub resources: bool,
    /// Wrecks, markers and anything else.
    pub other: bool,
    /// Walls, tubes and the mission's other tile edits.
    pub walls: bool,
    /// Owners whose objects are hidden.
    pub hidden_owners: BTreeSet<i32>,
}

impl Default for Layers {
    fn default() -> Self {
        Layers { objects: true, art: true, vehicles: true, structures: true, resources: true, other: true, walls: true, hidden_owners: BTreeSet::new() }
    }
}

impl Layers {
    /// Whether `o` is drawn: objects on, its kind's layer on, its owner not hidden.
    pub fn shows(&self, o: &Object) -> bool {
        self.objects
            && !self.hidden_owners.contains(&o.owner)
            && match o.kind {
                Kind::Vehicle => self.vehicles,
                Kind::Structure => self.structures,
                Kind::Resource => self.resources,
                Kind::Wreckage | Kind::Other => self.other,
            }
    }
}

/// The player colours' mid tones (blue, red, green, yellow, cyan, magenta); Gaia is white.
pub fn player_rgb(color: Option<u8>) -> [u8; 3] {
    /// RGB by player colour number.
    const RAMP: [[u8; 3]; 6] = [[60, 90, 230], [220, 50, 40], [40, 190, 60], [230, 210, 40], [40, 210, 220], [210, 60, 210]];
    color.map_or([240, 240, 240], |c| RAMP[c as usize % 6])
}

/// The player colour's name, as `player_rgb` gives it.
pub fn color_name(color: Option<u8>) -> &'static str {
    match color {
        Some(c) => ["blue", "red", "green", "yellow", "cyan", "magenta"][c as usize % 6],
        None => "none",
    }
}

/// A dot's fill: resources yellow, wrecks and the rest grey, units in their owner's colour.
pub fn dot_rgb(o: &Object) -> [u8; 3] {
    match o.kind {
        Kind::Resource => [250, 200, 60],
        Kind::Wreckage | Kind::Other => [180, 180, 180],
        _ => player_rgb(o.color),
    }
}

/// The art to draw for an object and the map pixel (x, y) its hotspot is anchored to.
pub type Anchored = (Look, (f32, f32));

/// The objects `layers` shows, in drawing order, each with the art to try for it and the map pixel that art is
/// anchored to, in map pixels (`None`: always a dot). Beacons, vents and fumaroles lie on the ground, under everything else; later
/// rows draw over earlier ones, as in the game.
pub fn plan<'a>(setup: &'a Setup, layers: &Layers) -> Vec<(&'a Object, Option<Anchored>)> {
    // A vehicle parked in a Garage is off the map.
    let mut order: Vec<&Object> = setup.objects.iter().filter(|o| layers.shows(o) && o.garage.is_none()).collect();
    // Resources first, then top to bottom and left to right, so lower objects draw over higher ones.
    order.sort_by_key(|o| (o.kind != Kind::Resource, o.pixel.1, o.pixel.0));
    // The dock pads of Agridomes, Storage Tanks and Garages and a mine's base are a second drawing under the structure,
    // drawn before any object (MapObjDrawList::Draw 0x49E990, via op2-rs's `pick::overlay`); a mission-placed
    // structure stands in state 0, built.
    let pads = order.iter().filter(|o| o.kind == Kind::Structure).filter_map(|o| {
        let pad = op2_formats::pick::overlay(&o.type_name, 0, o.plymouth)?;
        Some((*o, Some((Look::Frame { anim: pad.anim, frame: 0, tint: o.color }, (o.pixel.0 as f32, o.pixel.1 as f32)))))
    });
    let pads: Vec<_> = pads.collect();
    pads.into_iter()
        .chain(order.into_iter().map(|o| {
            let look = match o.kind {
                Kind::Vehicle | Kind::Structure => Some((Look::of(o), (o.pixel.0 as f32, o.pixel.1 as f32))),
                // A resource's art is its beacon's animation frame, anchored to the middle of its 32-pixel tile.
                Kind::Resource => setup.beacons.iter().find(|b| b.tile == o.tile).map(|b| {
                    (Look::Frame { anim: b.anim, frame: b.frame, tint: b.tint }, (b.tile.0 as f32 * 32.0 + 16.0, b.tile.1 as f32 * 32.0 + 16.0))
                }),
                _ => None,
            };
            (o, look)
        }))
        .collect()
}
