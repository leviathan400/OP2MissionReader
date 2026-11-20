//! Object art: op2-formats composes a unit's frame from `op2_art.prt` and `OP2_ART.BMP` (chassis plus turret,
//! coloured for its owner) with the animation tables taken from Outpost2.exe.

use std::sync::Arc;

use op2_formats::exe::GameExe;
use op2_formats::units;
use op2_formats::{GameFiles, Sprite, Sprites, Tint};

use crate::setup::{Kind, Object};

/// One distinct look.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Look {
    /// A vehicle or a guard post: its type, weapon, colony, direction column (0..16) and colour.
    Unit { code: String, weapon: Option<String>, plymouth: bool, column: u8, color: Option<u8> },
    /// A structure as a mission places it.
    Building { code: String, plymouth: bool, color: Option<u8> },
    /// One frame of an animation (beacons, vents, fumaroles).
    Frame { anim: usize, frame: usize, tint: Option<u8> },
}

impl Look {
    /// How `o` is drawn: vehicles and guard posts turn to face their direction, other structures don't.
    pub fn of(o: &Object) -> Look {
        let code = o.type_name.clone();
        if o.kind == Kind::Vehicle || code == "TOWER_GUARD" {
            // The mission's rotation is in eighths of a turn; the art has sixteen columns.
            let column = units::direction_index((o.rotation * 32) as u8) as u8;
            Look::Unit { code, weapon: o.weapon.clone(), plymouth: o.plymouth, column, color: o.color }
        } else {
            Look::Building { code, plymouth: o.plymouth, color: o.color }
        }
    }
}

/// The game's art.
pub struct Art {
    /// op2_art.prt's animations and the bitmaps they draw from.
    sprites: Sprites,
    /// The game's tables, among them which animations each unit type draws.
    exe: Arc<GameExe>,
}

impl Art {
    /// Reads the art files and Outpost2.exe's tables from the game folder.
    pub fn load(files: &GameFiles) -> Result<Art, op2_formats::Error> {
        Ok(Art { sprites: Sprites::load(files)?, exe: GameExe::of(files)? })
    }

    /// The loaded art itself (op2_art.prt and its bitmaps), for drawing frames directly (`exact`).
    pub fn sprites(&self) -> &Sprites {
        &self.sprites
    }

    /// The look's image and hotspot, `None` when it has no art.
    pub fn compose(&self, look: &Look) -> Option<Sprite> {
        // No colour keeps the bitmap's stored palette (Gaia's objects); a colour recolours the player-coloured pixels.
        let tint = |c: &Option<u8>| c.map_or(Tint::Stored, Tint::Player);
        let table = &self.exe.units;
        match look {
            Look::Unit { code, weapon, plymouth, column, color } => {
                // A guard post's Starflare and Supernova are their own weapons, but the art is listed under the tanks'.
                let weapon = weapon.as_deref().map(|w| match w {
                    "TURRET_DESTRUCT" => "SELF_DESTRUCT",
                    "TURRET_DESTRUCT_ADV" => "SELF_DESTRUCT_ADV",
                    w => w,
                });
                let turret = weapon.and_then(|w| table.turret(w));
                if code == "TOWER_GUARD" {
                    let post = turret?.for_chassis("TOWER_GUARD")?;
                    return self.sprites.frame_rgba_aligned(post.column(*plymouth, *column as usize), 0, tint(color)).ok();
                }
                let art = table.vehicle(code)?;
                let rotation = units::column_rotation(*column as usize);
                // A ConVec's "weapon" is its cargo kit, which has no turret art: those draw the chassis alone.
                match turret.filter(|t| t.for_chassis(&art.code).is_some()) {
                    Some(t) => units::tank_sprite(&self.sprites, art, t, *plymouth, rotation, tint(color)).ok(),
                    None => units::chassis_sprite(&self.sprites, art, *plymouth, rotation, 0, tint(color)).ok(),
                }
            }
            Look::Building { code, plymouth, color } => {
                // The last frame of the construction animation, which is how a mission-placed structure stands
                // (op2-engine, from harness traces).
                let anim = table.building(code)?.slots(*plymouth)[0];
                if anim == units::NONE {
                    return None;
                }
                let last = self.sprites.animation(anim as usize)?.frames.len().checked_sub(1)?;
                self.sprites.frame_rgba_aligned(anim as usize, last, tint(color)).ok()
            }
            Look::Frame { anim, frame, tint: t } => self.sprites.frame_rgba_aligned(*anim, *frame, tint(t)).ok(),
        }
    }
}
