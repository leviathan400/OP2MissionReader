//! Running a mission's setup: op2-missionhost maps the DLL into its x86 sandbox and calls `DllMain` and `InitProc`,
//! whose calls to the game API (`TethysGame::CreateUnit`, `CreateBeacon`, `CreateWallOrTube`...) land in its World.
//! What the World holds afterwards is the mission's starting state.

use std::path::Path;

use anyhow::Result;
use op2_missionhost::names;
use op2_missionhost::runtime::{Mission, Options, is_multiplayer};
use serde::Serialize;

/// Choices the game makes before `InitProc` runs that change what a mission creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartOptions {
    /// 0 easy, 1 normal, 2 hard.
    pub difficulty: i32,
    /// Players in a multiplayer start (`None` for other missions).
    pub players: Option<u8>,
}

impl StartOptions {
    /// Normal difficulty, and every slot of a multiplayer map filled.
    pub fn default_for(mission_type: i32, num_players: i32) -> Self {
        let players = is_multiplayer(mission_type).then(|| num_players.clamp(1, 6) as u8);
        StartOptions { difficulty: 1, players }
    }
}

/// Which list and drawing layer an object belongs to, decided by its map id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Kind {
    Vehicle,
    Structure,
    /// Mining beacons, magma vents, fumaroles.
    Resource,
    Wreckage,
    /// Everything else a mission can place: markers, disasters' objects.
    Other,
}

/// One unit, structure, beacon or other object the setup created, as it stands when the setup is over.
#[derive(Debug, Clone, Serialize)]
pub struct Object {
    /// The game's object index.
    pub id: u32,
    pub kind: Kind,
    /// The `map_id`'s name as the SDK spells it (`CON_TRUCK`, `BEACON`, `TOWER_GUARD`...).
    #[serde(rename = "type")]
    pub type_name: String,
    /// The `map_id`.
    #[serde(skip)]
    pub type_id: i32,
    /// The owning player's number ([`Player::id`]); one that matches none of [`Setup::players`] is Gaia.
    pub owner: i32,
    /// File tile (the map file's coordinates; the game's x is this plus the map padding).
    pub tile: (i32, i32),
    /// The map pixel the object's art is anchored to, op2-sim's: a vehicle's tile centre; a structure's footprint
    /// centre, on a tile edge along an even side (a 2x2 at its tile's top-left corner, a 3x3 at its tile's centre).
    pub pixel: (i32, i32),
    /// The file tiles a structure covers, top-left and bottom-right (0x408970: its size centred on `tile`, an even
    /// side's extra tile up and left); a vehicle's own tile.
    #[serde(skip)]
    pub footprint: ((i32, i32), (i32, i32)),
    /// The owner's colony (false for Gaia).
    pub plymouth: bool,
    /// The owner's colour (0..=5, blue, red, green, yellow, cyan, magenta); `None` for Gaia.
    pub color: Option<u8>,
    /// `weapon_id`'s name when it is set (above 0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weapon: Option<String>,
    /// The weapon's `map_id`, a ConVec's kit, or a wreck's tech number, as the mission gave it.
    #[serde(skip)]
    pub weapon_id: i32,
    /// Hit points left, 0.0..=1.0 (1.0 where op2-rs's simulation doesn't know the type).
    #[serde(skip)]
    pub health: f32,
    /// Facing as the mission gave it, in eighths of a turn (0..=7).
    pub rotation: i32,
    /// Headlights on.
    pub lights: bool,
    /// What some creators add: a beacon's ore type and yield, a truck's cargo, a wreck's tech...
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub extra: std::collections::BTreeMap<String, i64>,
    /// What it holds, as label and value: a Cargo Truck's load, a Structure Factory's or Spaceport's bays, a Garage's
    /// vehicles. Empty for anything else.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cargo: Vec<(&'static str, String)>,
    /// The Garage (object id) this vehicle is parked in, off the map.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub garage: Option<u32>,
}

/// One wall or tube tile the setup placed (`CreateWallOrTube`).
#[derive(Debug, Clone, Serialize)]
pub struct Wall {
    /// The `map_id`'s name (`TUBE`, `WALL`, `WALL_LAVA`, `WALL_MICROBE`).
    #[serde(rename = "type")]
    pub type_name: String,
    /// File tile.
    pub tile: (i32, i32),
}

/// One of the game's players as the setup left it.
#[derive(Debug, Clone, Serialize)]
pub struct Player {
    /// The player number objects' `owner` refers to.
    pub id: i32,
    /// Eden; Plymouth when false.
    pub eden: bool,
    /// A human player; an AI when false.
    pub human: bool,
    /// The colour index as the mission set it (0..=5, blue, red, green, yellow, cyan, magenta).
    pub color: i32,
    /// The starting state the setup gave the player (`Player::SetKids`, `SetOre`, `MarkResearchComplete`...).
    pub start: PlayerStart,
}

/// A player's people, stores and research as the setup set them.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PlayerStart {
    pub kids: i32,
    pub workers: i32,
    pub scientists: i32,
    /// Common ore in storage.
    pub ore: i32,
    pub rare_ore: i32,
    pub food: i32,
    /// Solar power satellites in orbit.
    pub solar_sats: i32,
    pub tech_level: i32,
    /// Research topics marked complete, by tech id (names in [`Setup::tech_names`]).
    pub techs: Vec<i32>,
    /// The player numbers this player is allied with.
    pub allies: Vec<i32>,
    /// File tile the view is centred on, if the mission set one.
    pub view: Option<(i32, i32)>,
    /// 0 (great) ..= 4 (rotten).
    pub morale_level: i32,
}

impl PlayerStart {
    /// The player's people and stores, as label and value.
    pub fn rows(&self) -> Vec<(&'static str, String)> {
        /// `morale_level`'s words.
        const MORALE: [&str; 5] = ["great", "good", "fair", "poor", "rotten"];
        vec![
            ("Workers", self.workers.to_string()),
            ("Scientists", self.scientists.to_string()),
            ("Kids", self.kids.to_string()),
            ("Common ore", self.ore.to_string()),
            ("Rare ore", self.rare_ore.to_string()),
            ("Food", self.food.to_string()),
            ("Solar satellites", self.solar_sats.to_string()),
            ("Morale", MORALE.get(self.morale_level as usize).unwrap_or(&"?").to_string()),
            ("Research done", self.techs.len().to_string()),
        ]
    }
}

/// How a mining beacon, magma vent or fumarole looks at the start (op2-sim's `looks::map_object`).
#[derive(Debug, Clone, Serialize)]
pub struct BeaconLook {
    /// File tile.
    pub tile: (i32, i32),
    /// Index into `op2_art.prt`'s animations.
    pub anim: usize,
    /// Frame of that animation.
    pub frame: usize,
    /// Player ramp the art is drawn with: none for ore beacons, player 1's for vents and fumaroles (0x4058DD).
    pub tint: Option<u8>,
}

/// A mission's starting state: what its `DllMain` and `InitProc` created and set.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Setup {
    /// Every live object, in the order op2-missionhost's World lists them.
    pub objects: Vec<Object>,
    /// How the beacons, vents and fumaroles among `objects` are drawn; empty without op2-rs's simulation.
    pub beacons: Vec<BeaconLook>,
    /// Every tile's graphic (mapping index) after the setup, row by row: walls, tubes and the mission's tile edits
    /// included. `None` without op2-rs's simulation (no sheets in the install).
    #[serde(skip)]
    pub graphics: Option<Vec<u16>>,
    /// Walls and tubes, one per tile.
    pub walls: Vec<Wall>,
    /// The game's players (the World's player count, 1 to 7), in the World's order.
    pub players: Vec<Player>,
    /// Where the mission's setup stopped, if it didn't finish (`DllMain: ...`, `InitProc: ...`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
    /// Game API functions the mission called that op2-missionhost only stubs, with call counts.
    pub unimplemented: Vec<(String, u64)>,
    /// Units op2-rs's simulation couldn't place (they're still listed).
    pub spawn_errors: Vec<String>,
    /// Game-wide switches the setup set, by their API name (`SetDaylightMoves`, `ForceMoraleGood`...), with the last
    /// call's arguments.
    #[serde(skip)]
    pub options: std::collections::BTreeMap<String, String>,
    /// The music playlist (song ids, the index to repeat from), if the setup set one.
    #[serde(skip)]
    pub music: Option<(Vec<u32>, u32)>,
    /// Victory and failure conditions and the other triggers the setup created.
    pub triggers: Vec<crate::triggers::Trigger>,
    /// The computer players' AI groups the setup created.
    pub groups: Vec<crate::groups::Group>,
    /// Research topic names by id, from the mission's tech tree file (empty if it couldn't be read).
    #[serde(skip)]
    pub tech_names: std::collections::BTreeMap<i32, String>,
}

/// Loads the mission and runs its setup. An error is a DLL that couldn't be loaded; a setup that stops part-way is
/// `Setup::failed`, with what it created up to there.
pub fn run(path: &Path, start: StartOptions) -> Result<Setup> {
    let opts = Options { call_log: false, difficulty: start.difficulty, players: start.players, ..Options::from_env() };
    Ok(run_with(path, &opts)?.0)
}

/// `run` with op2-missionhost's options given whole (a multiplayer lobby's start, the seed, the call log), and the
/// game API calls the setup made (empty unless `opts.call_log`).
pub fn run_with(path: &Path, opts: &Options) -> Result<(Setup, Vec<op2_missionhost::runtime::CallRec>)> {
    let mut ms = Mission::load(path, opts)?;
    let failed = ms.start().err().map(|(phase, why)| format!("{phase}: {why}"));
    // The World's units, walls and views are in game tiles (x padded by `w.pad`) and turned into file tiles here;
    // op2-sim's beacon tiles are file tiles already.
    let w = &ms.rt.world;
    let player = |owner: i32| w.players.get(..w.num_players.clamp(1, 7) as usize).and_then(|ps| ps.iter().find(|p| p.id == owner));
    let objects = w
        .live_units()
        .map(|u| {
            let kind = kind(u.type_id);
            let (x, y) = (u.x - w.pad, u.y);
            let p = player(u.owner);
            // A structure's size in tiles from op2-sim's stats; 1x1 for anything else or without the simulation.
            let (fw, fh) = w.sim.as_ref().and_then(|sim| Some(sim.rules().stats(sim.unit(u.sim?)?.kind).size)).filter(|_| kind == Kind::Structure).unwrap_or((1, 1));
            // The damage the setup dealt, against the type's hit points.
            let health = w
                .sim
                .as_ref()
                .zip(u.sim)
                .and_then(|(sim, id)| Some(sim.rules().stats(sim.unit(id)?.kind).hp))
                .filter(|&hp| hp > 0)
                .map_or(1.0, |hp| (1.0 - u.damage as f32 / hp as f32).clamp(0.0, 1.0));
            Object {
                id: u.id,
                kind,
                type_name: u.type_name.clone(),
                type_id: u.type_id,
                owner: u.owner,
                tile: (x, y),
                footprint: ((x - fw / 2, y - fh / 2), (x - fw / 2 + fw - 1, y - fh / 2 + fh - 1)),
                pixel: w.sim.as_ref().zip(u.sim).and_then(|(sim, id)| sim.unit(id)).map_or((x * 32 + 16, y * 32 + 16), |s| (s.px, s.py)),
                plymouth: p.is_some_and(|p| !p.eden),
                color: p.map(|p| p.color.clamp(0, 5) as u8),
                weapon: (u.weapon > 0).then(|| names::map_id_name(u.weapon)),
                weapon_id: u.weapon,
                health,
                // The heading is 0..255; missions give eighths of a turn (heading = rotation * 32).
                rotation: (u.heading as i32 + 16) / 32 % 8,
                lights: u.lights,
                extra: u.extra.clone(),
                cargo: cargo(&u.type_name, &u.extra),
                garage: None,
            }
        })
        .collect();
    let (beacons, graphics) = match &w.sim {
        Some(sim) => {
            let beacons = sim
                .beacons()
                .iter()
                .filter_map(|b| {
                    use op2_sim::colony::BeaconKind;
                    use op2_sim::looks::id;
                    // Ore beacons as a player who has surveyed them sees them, so common and rare (and their yield
                    // bars) tell apart; the game itself hides rare ore until it's researched and shows every
                    // unsurveyed beacon alike.
                    let (anim_id, frame, tint) = match b.kind {
                        BeaconKind::Ore { .. } if b.removed => return None,
                        BeaconKind::Ore { rare, bar, .. } => {
                            let bar = bar.clamp(0, 2) as usize;
                            (if rare { id::BEACON_RARE[bar] } else { id::BEACON_COMMON[bar] }, 0, None)
                        }
                        _ => {
                            let look = op2_sim::looks::map_object(sim, b, 0, true)?;
                            (look.anim_id, look.frame, Some(1))
                        }
                    };
                    // Anim ids count from ANIM_ID_BASE; `op2_art.prt`'s list from 0.
                    let anim = (anim_id as usize).checked_sub(op2_formats::units::ANIM_ID_BASE as usize)?;
                    Some(BeaconLook { tile: b.tile, anim, frame, tint })
                })
                .collect();
            let t = sim.terrain();
            let graphics = (0..t.height as i32).flat_map(|y| (0..t.width as i32).map(move |x| t.graphic(x, y))).collect();
            (beacons, Some(graphics))
        }
        None => (Vec::new(), None),
    };
    // The yield the game settled on for each ore beacon (a mission's -1 lets the game choose).
    let mut objects: Vec<Object> = objects;
    if let Some(sim) = &w.sim {
        for b in sim.beacons() {
            if let op2_sim::colony::BeaconKind::Ore { bar, .. } = b.kind
                && let Some(o) = objects.iter_mut().find(|o| o.type_name == "BEACON" && o.tile == b.tile)
            {
                // op2-sim's `bar` counts 0 for 3 bars up to 2 for 1 bar.
                o.extra.insert("bars".into(), 3 - bar.clamp(0, 2) as i64);
            }
        }
        // Vehicles the setup parked in a Garage (`Unit::PutInGarage`): op2-sim holds which bay each is in.
        let parked: Vec<(u32, usize, u32)> = w
            .live_units()
            .filter(|g| g.type_name == "GARAGE")
            .filter_map(|g| Some((g.id, sim.garage_bays(g.sim?))))
            .flat_map(|(g, bays)| bays.into_iter().enumerate().filter_map(move |(bay, v)| Some((g, bay, v?))))
            .filter_map(|(g, bay, v)| Some((g, bay, w.live_units().find(|u| u.sim == Some(v))?.id)))
            .collect();
        for (g, bay, v) in parked {
            let Some(vehicle) = objects.iter_mut().find(|o| o.id == v) else { continue };
            let what = vehicle.describe();
            vehicle.garage = Some(g);
            if let Some(garage) = objects.iter_mut().find(|o| o.id == g) {
                garage.cargo.retain(|(k, _)| *k != "Bays");
                garage.cargo.push((BAYS[bay.min(5)], what));
            }
        }
    }
    let walls = w.settings.walls.iter().map(|(x, y, t)| Wall { type_name: t.clone(), tile: (x - w.pad, *y) }).collect();
    let players = w.players[..w.num_players.clamp(1, 7) as usize]
        .iter()
        .map(|p| Player {
            id: p.id,
            eden: p.eden,
            human: p.human,
            color: p.color,
            start: PlayerStart {
                kids: p.kids,
                workers: p.workers,
                scientists: p.scientists,
                ore: p.ore,
                rare_ore: p.rare_ore,
                food: p.food,
                solar_sats: p.solar_sats,
                tech_level: p.tech_level,
                techs: p.techs.iter().copied().collect(),
                allies: p.allies.iter().copied().collect(),
                view: p.view.map(|(x, y)| (x - w.pad, y)),
                morale_level: p.morale_level,
            },
        })
        .collect();
    let tech_names = tech_names(path, &ms.desc.techtree_name);
    let triggers = crate::triggers::collect(w, &objects, &tech_names);
    let groups = crate::groups::collect(w, &objects);
    let options = w.settings.values.clone();
    let music = w.music.as_ref().map(|m| (m.songs.clone(), m.repeat));
    let mut unimplemented: Vec<(String, u64)> = ms.rt.unimplemented.iter().map(|(k, v)| (k.clone(), *v)).collect();
    unimplemented.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let setup = Setup {
        objects,
        beacons,
        graphics,
        walls,
        players,
        failed,
        unimplemented,
        spawn_errors: w.sim_spawn_errors.clone(),
        options,
        music,
        triggers,
        groups,
        tech_names,
    };
    Ok((setup, std::mem::take(&mut ms.rt.log)))
}

/// The research topics' names from the mission's tech tree file: next to the DLL (missions that bring their own),
/// else from the game's files.
pub fn tech_names(dll: &Path, file: &str) -> std::collections::BTreeMap<i32, String> {
    if file.is_empty() {
        return Default::default();
    }
    let beside = dll.parent().and_then(|dir| std::fs::read_dir(dir).ok()).and_then(|entries| {
        entries.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(file)).and_then(|e| std::fs::read(e.path()).ok())
    });
    let data = beside.or_else(|| {
        let files = op2_formats::GameFiles::open(op2_formats::default_game_dir()).ok()?;
        files.read(file).ok().map(|(d, _)| d)
    });
    let Some(data) = data else { return Default::default() };
    // Tech trees are Windows-1252.
    let text: String = data.iter().map(|&b| b as char).collect();
    op2_formats::sheets::parse_techs(&text).map(|ts| ts.into_iter().map(|t| (t.id as i32, t.name)).collect()).unwrap_or_default()
}

/// Which list and layer a map id goes in.
fn kind(type_id: i32) -> Kind {
    match names::map_id_name(type_id).as_str() {
        _ if names::is_vehicle(type_id) => Kind::Vehicle,
        _ if names::is_building(type_id) => Kind::Structure,
        "BEACON" | "MAGMA_VENT" | "FUMAROLE" => Kind::Resource,
        "WRECK_FLAG" => Kind::Wreckage,
        _ => Kind::Other,
    }
}

/// What a setup placed of the map's resources: ore beacons by ore and yield, magma vents, fumaroles and wrecks.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Resources {
    /// Common ore beacons.
    pub common: Yields,
    /// Rare ore beacons.
    pub rare: Yields,
    pub magma_vents: u32,
    pub fumaroles: u32,
    /// Wreckage (`WRECK_FLAG` objects).
    pub wrecks: u32,
}

/// Ore beacons by yield; `random` where the mission let the game choose the yield.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Yields {
    /// All beacons of this ore; the sum of the other fields.
    pub total: u32,
    /// 1-bar (lowest yield) beacons.
    pub bar1: u32,
    pub bar2: u32,
    /// 3-bar (highest yield) beacons.
    pub bar3: u32,
    /// Beacons without a settled yield (no `bars` in [`Object::extra`]).
    pub random: u32,
}

impl Yields {
    /// Counts one beacon by its `bars` (1 to 3); anything else counts as `random`.
    fn add(&mut self, bars: Option<i64>) {
        self.total += 1;
        match bars {
            Some(1) => self.bar1 += 1,
            Some(2) => self.bar2 += 1,
            Some(3) => self.bar3 += 1,
            _ => self.random += 1,
        }
    }

    /// "12 common ore (8 3-bar, 4 random)" with `ore` as the name, or None when there are none.
    fn describe(&self, ore: &str) -> Option<String> {
        if self.total == 0 {
            return None;
        }
        let parts: Vec<String> = [(self.bar3, "3-bar"), (self.bar2, "2-bar"), (self.bar1, "1-bar"), (self.random, "random")]
            .iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, name)| format!("{n} {name}"))
            .collect();
        Some(format!("{} {ore} ({})", self.total, parts.join(", ")))
    }
}

impl Resources {
    /// One line: "12 common ore (8 3-bar, 4 random), 4 rare ore (4 2-bar), 2 magma vents", or None for nothing.
    pub fn describe(&self) -> Option<String> {
        let mut parts: Vec<String> = [self.common.describe("common ore"), self.rare.describe("rare ore")].into_iter().flatten().collect();
        for (n, one, many) in [(self.magma_vents, "magma vent", "magma vents"), (self.fumaroles, "fumarole", "fumaroles"), (self.wrecks, "wreck", "wrecks")] {
            if n > 0 {
                parts.push(format!("{n} {}", if n == 1 { one } else { many }));
            }
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

impl Setup {
    /// The resources the setup placed, counted.
    pub fn resources(&self) -> Resources {
        let mut r = Resources::default();
        for ob in &self.objects {
            match ob.type_name.as_str() {
                "BEACON" => {
                    let bars = ob.extra.get("bars").copied();
                    if ob.extra.get("oreType").is_some_and(|&t| t == 1) { r.rare.add(bars) } else { r.common.add(bars) }
                }
                "MAGMA_VENT" => r.magma_vents += 1,
                "FUMAROLE" => r.fumaroles += 1,
                "WRECK_FLAG" => r.wrecks += 1,
                _ => {}
            }
        }
        r
    }

    /// How many objects of this kind the setup made.
    pub fn count(&self, kind: Kind) -> usize {
        self.objects.iter().filter(|o| o.kind == kind).count()
    }

    /// "12 vehicles, 4 structures, 6 resources, 30 walls/tubes", plus how the setup ended when it didn't finish.
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = [
            (Kind::Vehicle, "vehicles"),
            (Kind::Structure, "structures"),
            (Kind::Resource, "resources"),
            (Kind::Wreckage, "wrecks"),
            (Kind::Other, "other"),
        ]
        .iter()
        .map(|(k, name)| (self.count(*k), name))
        .filter(|(n, _)| *n > 0)
        .map(|(n, name)| format!("{n} {name}"))
        .collect();
        if !self.walls.is_empty() {
            parts.push(format!("{} walls/tubes", self.walls.len()));
        }
        let mut s = if parts.is_empty() { "nothing created".to_string() } else { parts.join(", ") };
        if let Some(f) = &self.failed {
            s += &format!("  [stopped in {f}]");
        }
        s
    }

    /// Prints the setup as `inspect --objects` shows it: players, objects, triggers and groups.
    pub fn print(&self) {
        println!("  Setup:         {}", self.summary());
        if let Some(r) = self.resources().describe() {
            println!("  Resources:     {r}");
        }
        for p in &self.players {
            println!(
                "  Player {}:      {}, {}, colour {}",
                p.id,
                if p.eden { "Eden" } else { "Plymouth" },
                if p.human { "human" } else { "AI" },
                p.color
            );
            let rows: Vec<String> = p.start.rows().iter().map(|(k, v)| format!("{} {v}", k.to_lowercase())).collect();
            println!("                 {}", rows.join(", "));
        }
        for o in &self.objects {
            println!("    {:>4}  player {}  ({:>3},{:>3})  {:?}  {}", o.id, o.owner, o.tile.0, o.tile.1, o.kind, o.describe());
        }
        if !self.walls.is_empty() {
            let mut counts = std::collections::BTreeMap::new();
            for w in &self.walls {
                *counts.entry(&w.type_name).or_insert(0) += 1;
            }
            let list: Vec<String> = counts.iter().map(|(t, n)| format!("{n} {t}")).collect();
            println!("    walls/tubes: {}", list.join(", "));
        }
        if !self.triggers.is_empty() {
            println!("  Triggers:      {}", self.triggers.len());
        }
        for t in &self.triggers {
            let mut line = format!("    #{:<4} {}: {}", t.id, t.kind, t.what);
            if let Some(o) = &t.objective {
                line += &format!("  \"{}\"", o.text);
            }
            if let Some(c) = &t.callback {
                line += &format!("  -> {c}");
            }
            println!("{line}");
        }
        if !self.groups.is_empty() {
            println!("  AI groups:     {}", self.groups.len());
        }
        for g in &self.groups {
            let mut line = format!("    {}  player {}, {} units", g.title(), g.owner, g.members.len());
            for (label, value) in &g.orders {
                line += &format!("; {}: {value}", label.to_lowercase());
            }
            if !g.wants.is_empty() {
                let wants: Vec<String> = g.wants.iter().map(|w| format!("{} {}", w.count, w.unit)).collect();
                line += &format!("; wants {}", wants.join(", "));
            }
            if !g.plans.is_empty() {
                line += &format!("; {} planned", g.plans.len());
            }
            println!("{line}");
        }
        for e in &self.spawn_errors {
            println!("  note: {e}");
        }
        if !self.unimplemented.is_empty() {
            let list: Vec<String> = self.unimplemented.iter().map(|(f, n)| format!("{f} x{n}")).collect();
            println!("  note: stubbed game calls: {}", list.join(", "));
        }
    }
}

impl Object {
    /// The type as a reader would say it, with what matters about this one (weapon, beacon ore).
    pub fn describe(&self) -> String {
        let mut s = self.type_name.clone();
        if let Some(w) = &self.weapon {
            s += &format!(" ({w})");
        }
        if self.type_name == "BEACON" {
            let ore = match self.extra.get("oreType") {
                Some(0) => "common ore",
                Some(1) => "rare ore",
                _ => "?",
            };
            match self.extra.get("bars") {
                Some(n) => s += &format!(" ({ore}, {n}-bar)"),
                // CreateBeacon's bar-yield argument as given (-1 lets the game choose).
                None => s += &format!(" ({ore}, yield code {})", self.extra.get("yield").copied().unwrap_or(-1)),
            }
        }
        match self.cargo.as_slice() {
            [] => {}
            [(_, v)] if v == "empty" => {}
            [("Cargo", load)] => s += &format!(" [{load}]"),
            bays => s += &format!(" [{} in bays]", bays.len()),
        }
        if self.garage.is_some() {
            s += " [in a garage]";
        }
        s
    }

    /// What the mission set on this object beyond its type and place, as label and value.
    pub fn details(&self) -> Vec<(&'static str, String)> {
        let mut d = Vec::new();
        if let Some(w) = &self.weapon {
            d.push((if self.type_name == "CON_TRUCK" { "Cargo" } else { "Weapon" }, w.clone()));
        }
        if self.kind == Kind::Vehicle {
            /// `rotation`'s eighths of a turn, from east clockwise.
            const FACING: [&str; 8] = ["east", "south-east", "south", "south-west", "west", "north-west", "north", "north-east"];
            d.push(("Facing", FACING[self.rotation as usize % 8].into()));
            d.push(("Lights", if self.lights { "on" } else { "off" }.into()));
        }
        d.extend(self.cargo.iter().cloned());
        if let Some(g) = self.garage {
            d.push(("Parked in", format!("garage #{g}")));
        }
        for (k, v) in &self.extra {
            match k.as_str() {
                "truckCargo" | "truckAmount" => {}
                _ if k.starts_with("bay") => {}
                "cargoWeapon" if *v > 0 => d.push(("Cargo weapon", names::map_id_name(*v as i32))),
                "oreType" => d.push(("Ore", if *v == 1 { "rare" } else { "common" }.into())),
                "bars" => d.push(("Yield", format!("{v}-bar"))),
                "yield" => d.push(("Yield as given", if *v < 0 { "random (-1)".into() } else { v.to_string() })),
                "variant" => d.push(("Variant", v.to_string())),
                "discovered" => d.push(("Discovered", if *v != 0 { "yes" } else { "no" }.into())),
                "marker" => d.push(("Marker", v.to_string())),
                "cargoWeapon" => {}
                _ => d.push(("Extra", format!("{k} = {v}"))),
            }
        }
        d
    }
}

/// Labels for a factory's or garage's bays.
const BAYS: [&str; 6] = ["Bay 1", "Bay 2", "Bay 3", "Bay 4", "Bay 5", "Bay 6"];

/// What a unit holds as the mission set it: a Cargo Truck's load (`SetTruckCargo`, or `CreateUnitBlock`'s cargo), a
/// Structure Factory's or Spaceport's bays (`SetFactoryCargo`: the item's map id above the weapon's). A Garage starts
/// empty; `run` adds the vehicles parked in it.
fn cargo(type_name: &str, extra: &std::collections::BTreeMap<String, i64>) -> Vec<(&'static str, String)> {
    let mut c = Vec::new();
    match type_name {
        "BIG_TRUCK" => {
            let load = extra.get("truckCargo").copied().unwrap_or(0);
            let amount = extra.get("truckAmount").copied().unwrap_or(0);
            c.push(("Cargo", if load == 0 { "empty".into() } else { format!("{}, {amount}", truck_cargo(load)) }));
        }
        "FACT_STRUCTURE" | "SPACEPORT" | "GARAGE" => {
            for (bay, name) in BAYS.iter().enumerate() {
                if let Some(v) = extra.get(&format!("bay{bay}")).filter(|&&v| v >> 16 > 0) {
                    let (item, weapon) = ((v >> 16) as i32, (v & 0xFFFF) as i32);
                    let mut s = names::map_id_name(item);
                    if weapon > 0 {
                        s += &format!(" ({})", names::map_id_name(weapon));
                    }
                    c.push((*name, s));
                }
            }
            if c.is_empty() {
                c.push(("Bays", "empty".into()));
            }
        }
        _ => {}
    }
    c
}

/// A cargo truck's load (the SDK's `TruckCargo`).
pub fn truck_cargo(v: i64) -> &'static str {
    match v {
        0 => "empty",
        1 => "food",
        2 => "common ore",
        3 => "rare ore",
        4 => "common metal",
        5 => "rare metal",
        6 => "common rubble",
        7 => "rare rubble",
        8 => "spaceport cargo",
        9 => "garbage",
        10 => "a unit",
        _ => "?",
    }
}

/// A game state written by OP2TestHarness's whole-map images (`state_<tick>.json`): a running game's map and objects,
/// as a [`Setup`] the exporter draws like a mission's start.
pub struct State {
    /// The state's tiles, players and objects; no walls, triggers or groups.
    pub setup: Setup,
    /// The map file the game loaded (the mission DLL's `MapName`).
    pub map_name: String,
    /// The state's `mission` field as the harness wrote it.
    pub mission: String,
    /// The game tick the state was taken at.
    pub tick: i64,
}

/// A running game's state from any source (a harness state, a saved game), as [`from_parts`] takes it.
#[derive(Debug, Clone, Default)]
pub struct StateParts {
    /// The map's size in tiles, and each file tile's mapping index (bits 5..16 of its tile word), row by row.
    pub width: i64,
    pub height: i64,
    pub graphics: Vec<u16>,
    pub players: Vec<StatePlayer>,
    pub objects: Vec<StateObject>,
    /// The map file the game loaded, the mission DLL, the tick.
    pub map_name: String,
    pub mission: String,
    pub tick: i64,
}

/// One player of a [`StateParts`].
#[derive(Debug, Clone, Copy, Default)]
pub struct StatePlayer {
    pub id: i32,
    pub eden: bool,
    pub human: bool,
    pub colour: i32,
}

/// One map object as the game had it.
#[derive(Debug, Clone, Default)]
pub struct StateObject {
    /// The game's object index.
    pub index: u32,
    /// The type's code name (`LIGHT_TANK`, `BEACON`; [`type_name`] gives it from a map id).
    pub type_name: String,
    pub owner: i32,
    /// Its pixel in file coordinates (the game's less the map's padding).
    pub x: i32,
    pub y: i32,
    /// Facing, 0-255.
    pub rotation: i32,
    /// A turret's weapon or a ConVec's kit (a map id).
    pub cargo: i32,
    /// A cargo truck's load: cargo type and amount.
    pub truck: Option<(i64, i64)>,
    /// A mining beacon's ore (0 common, 1 rare) and bar yield (0 three bars, 1 two, 2 one).
    pub beacon: Option<(i64, i64)>,
}

/// A map id's code name, as the game and a harness state spell it.
pub fn type_name(type_id: i32) -> String {
    names::map_id_name(type_id)
}

/// Whether a map object is a weapon's fire (a shot in flight or its blast: map ids 0x3B ACID to 0x4B BFG with the
/// child-entity flag 0x100), which a picture of the game leaves out, as OP2TestHarness's states do.
pub fn is_weapon_fire(type_id: i32, flags: u32) -> bool {
    flags & 0x100 != 0 && (0x3B..=0x4B).contains(&type_id)
}

/// Reads a harness state (`state_<tick>.json`) into a [`State`] through [`from_parts`].
pub fn from_state(json: &str) -> Result<State> {
    use anyhow::Context;
    let v: serde_json::Value = serde_json::from_str(json).context("not a harness state (JSON)")?;
    let int = |o: &serde_json::Value, k: &str| o.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
    let (width, height) = (int(&v, "width"), int(&v, "height"));
    let tiles = v.get("tiles").and_then(|t| t.as_array()).context("no tiles: not a harness state")?;
    anyhow::ensure!(tiles.len() as i64 == width * height, "{} tiles for a {width}x{height} map", tiles.len());
    let graphics: Vec<u16> = tiles.iter().map(|t| t.as_u64().unwrap_or(0) as u16).collect();
    let players = v
        .get("players")
        .and_then(|p| p.as_array())
        .map(|ps| {
            ps.iter()
                .map(|p| StatePlayer { id: int(p, "id") as i32, eden: int(p, "eden") != 0, human: int(p, "human") != 0, colour: int(p, "colour") as i32 })
                .collect()
        })
        .unwrap_or_default();
    let objects = v
        .get("objects")
        .and_then(|o| o.as_array())
        .into_iter()
        .flatten()
        .map(|o| {
            let type_name = o.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
            let truck = o.get("truck").and_then(|t| t.as_array()).map(|t| {
                (t.first().and_then(|x| x.as_i64()).unwrap_or(0), t.get(1).and_then(|x| x.as_i64()).unwrap_or(0))
            });
            let beacon = (type_name == "BEACON").then(|| (int(o, "ore"), int(o, "bar")));
            StateObject {
                index: int(o, "i") as u32,
                type_name,
                owner: int(o, "owner") as i32,
                x: int(o, "x") as i32,
                y: int(o, "y") as i32,
                rotation: int(o, "rot") as i32,
                cargo: int(o, "cargo") as i32,
                truck,
                beacon,
            }
        })
        .collect();
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    Ok(from_parts(StateParts { width, height, graphics, players, objects, map_name: text("map"), mission: text("mission"), tick: int(&v, "tick") }))
}

/// A running game's state as a [`State`] to draw: its tiles become [`Setup::graphics`], its objects [`Object`]s.
/// Facing is kept to eighths of a turn, as a mission gives it; hit points aren't known, so units are drawn whole.
pub fn from_parts(parts: StateParts) -> State {
    let players: Vec<Player> = parts
        .players
        .iter()
        .map(|p| Player { id: p.id, eden: p.eden, human: p.human, color: p.colour, start: PlayerStart::default() })
        .collect();
    let mut objects = Vec::new();
    let mut beacons = Vec::new();
    for o in parts.objects {
        let type_name = o.type_name;
        let type_id = names::map_id_of(&type_name).unwrap_or(0);
        let kind = kind(type_id);
        // File pixels (32 per tile, the map's padding already taken off), so the tile the pixel falls in is a file
        // tile.
        let (px, py) = (o.x, o.y);
        let tile = (px.div_euclid(32), py.div_euclid(32));
        let owner = o.owner;
        let p = players.iter().find(|p| p.id == owner);
        let cargo = o.cargo;
        // The game's one field: a turret's weapon (Lynx, Panther, Tiger, Guard Post), a ConVec's kit, a truck's load.
        let turret = matches!(type_name.as_str(), "LIGHT_TANK" | "MED_TANK" | "BIG_TANK" | "TOWER_GUARD");
        let mut extra = std::collections::BTreeMap::new();
        if let Some((cargo_type, amount)) = o.truck {
            extra.insert("truckCargo".to_string(), cargo_type);
            extra.insert("truckAmount".to_string(), amount);
        }
        // Beacons, vents and fumaroles also get a look, as op2-sim's would be in `run_with`.
        if type_name == "BEACON" {
            let (ore, bar) = o.beacon.unwrap_or((0, 0));
            let bar = bar.clamp(0, 2) as usize;
            let rare = ore == 1;
            extra.insert("bars".to_string(), 3 - bar as i64);
            let anim_id = if rare { op2_sim::looks::id::BEACON_RARE[bar] } else { op2_sim::looks::id::BEACON_COMMON[bar] };
            if let Some(anim) = (anim_id as usize).checked_sub(op2_formats::units::ANIM_ID_BASE as usize) {
                beacons.push(BeaconLook { tile, anim, frame: 0, tint: None });
            }
        } else if matches!(type_name.as_str(), "MAGMA_VENT" | "FUMAROLE") {
            let anim_id = if type_name == "FUMAROLE" { op2_sim::looks::id::FUMAROLE } else { op2_sim::looks::id::MAGMA_VENT };
            if let Some(anim) = (anim_id as usize).checked_sub(op2_formats::units::ANIM_ID_BASE as usize) {
                beacons.push(BeaconLook { tile, anim, frame: 0, tint: Some(1) });
            }
        }
        objects.push(Object {
            id: o.index,
            kind,
            cargo: crate::setup::cargo(&type_name, &extra),
            type_name,
            type_id,
            owner,
            tile,
            pixel: (px, py),
            footprint: (tile, tile),
            plymouth: p.is_some_and(|p| !p.eden),
            color: p.map(|p| p.color.clamp(0, 5) as u8),
            weapon: (turret && cargo > 0).then(|| names::map_id_name(cargo)),
            weapon_id: cargo,
            health: 1.0,
            // A 0..255 heading, rounded to the nearest eighth as in `run_with`.
            rotation: (o.rotation + 16) / 32 % 8,
            lights: true,
            extra,
            garage: None,
        });
    }
    let setup = Setup {
        objects,
        beacons,
        graphics: Some(parts.graphics),
        walls: Vec::new(),
        players,
        failed: None,
        unimplemented: Vec::new(),
        spawn_errors: Vec::new(),
        options: Default::default(),
        music: None,
        triggers: Vec::new(),
        groups: Vec::new(),
        tech_names: Default::default(),
    };
    State { setup, map_name: parts.map_name, mission: parts.mission, tick: parts.tick }
}
