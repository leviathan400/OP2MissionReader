//! `op2dll lua`: a mission DLL converted to an OP2Lua mission (`<Base>.placement.lua`, `<Base>.lua` and a stub
//! `<Base>.dll`), written from what the mission does, not from its code. Single-player missions (campaign, colony
//! games, auto demos, tutorials) are `colony.rs`'s; this file converts multiplayer ones, as below.
//!
//! The mission's setup runs many times in op2-missionhost, started as a network game's lobby would start it: every
//! player count, each resource level, with and without initial vehicles, Eden and Plymouth, Day and Night and Morale
//! Steady on and off, and enough seeds to see every way its random rolls lay out and hand out the bases. Comparing
//! the runs sorts what it makes:
//! - a *variant* is one base layout, keyed by where its Command Center stands, with its units (read from the setup's
//!   own `CreateUnit` calls: the tiles it asked for), and the tubes, walls and beacons only it brings;
//! - variants close together form an *area* (a corner of the map, say): the stock maps roll one of several layouts
//!   for each area;
//! - which player gets which area, for each player count, is a table of what the runs gave out, and each area's
//!   variant is rolled from those seen, both with `game.rand` (so the same choices, not the same rolls);
//! - an area's variants that are one kit at different tiles (a Land Rush start) are one *spread* layout: the kit,
//!   relative to a start tile rolled from the tiles seen, or anywhere in their rectangle when they fill it;
//! - beacons that come and go are spot lists of which every game has the same number, area by area where that
//!   holds (spots moved between areas until it does), with the kinds made on them when every game makes the same;
//! - units only some resource levels get, the weapons Plymouth gets instead of Eden's, the lobby's initial vehicles
//!   (type and weapon by colony, and the spots they're dropped on), colonists, stores, research and Structure Factory
//!   kits by level, day and night, morale and the win (`victory.rs`), where lava may flow;
//! - what every run makes (beacons, tubes) goes in placement.lua.
//!
//! What it can't place in those patterns (triggers other than the win, objects nobody owns, differences it can't
//! explain) is listed as comments at the end of `<Base>.lua`, so nothing is dropped silently. Disasters and other
//! trigger functions are `timed.rs`'s; a computer player and its AI groups are `ai.rs`'s.
//!
//! Without `--simple` or a computer player, the setup is written as a replay of the original's instead
//! (`replay.rs`): the same draws and calls in the same order. The model above is the fallback when the replay can't
//! be made, and its win and timed functions are used either way.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result, bail};
use op2_missionhost::names::{MAP_IDS, is_building};
use op2_missionhost::runtime::{CallRec, NetSlot, NetStart, Options, is_multiplayer};

use crate::setup::Setup;

/// OP2Lua's type names by map id (Tethys `MapID`, the same numbering as op2-sim's codes), to the last space module.
const LUA_NAMES: [&str; 105] = [
    "None", "CargoTruck", "ConVec", "Spider", "Scorpion", "Lynx", "Panther", "Tiger", "RoboSurveyor", "RoboMiner",
    "GeoCon", "Scout", "RoboDozer", "EvacuationTransport", "RepairVehicle", "Earthworker", "SmallCapacityAirTransport",
    "Tube", "Wall", "LavaWall", "MicrobeWall", "CommonOreMine", "RareOreMine", "GuardPost", "LightTower",
    "CommonStorage", "RareStorage", "Forum", "CommandCenter", "MHDGenerator", "Residence", "RobotCommand", "TradeCenter",
    "BasicLab", "MedicalCenter", "Nursery", "SolarPowerArray", "RecreationFacility", "University", "Agridome", "DIRT",
    "Garage", "MagmaWell", "MeteorDefense", "GeothermalPlant", "ArachnidFactory", "ConsumerFactory", "StructureFactory",
    "VehicleFactory", "StandardLab", "AdvancedLab", "Observatory", "ReinforcedResidence", "AdvancedResidence",
    "CommonOreSmelter", "Spaceport", "RareOreSmelter", "GORF", "Tokamak", "AcidCloud", "EMP", "Laser", "Microwave",
    "RailGun", "RPG", "Starflare", "Supernova", "Starflare", "Supernova", "None", "ESG", "Stickyfoam", "ThorsHammer",
    // 73 is the Scorpion's weapon. 74 to 87 (weapons fire, disasters, wrecks, explosions) keep op2-missionhost's
    // names; the beacons (81 to 83) have OP2Lua's, for count triggers on them (Double Cross's lava waits on a magma
    // vent).
    "EnergyCannon", "None", "None", "None", "None", "None", "None", "None", "MiningBeacon", "MagmaVent", "Fumarole", "None", "None",
    "None", "None", "EDWARDSatellite", "SolarSatellite", "IonDriveModule",
    "FusionDriveModule", "CommandModule", "FuelingSystems", "HabitatRing", "SensorPackage", "Skydock", "StasisSystems",
    "OrbitalPackage", "PhoenixModule", "RareMetalsCargo", "CommonMetalsCargo", "FoodCargo", "EvacuationModule",
    "ChildrenModule",
];
/// OP2Lua's direction names by a unit's rotation from `CreateUnit`, in eighths of a turn from East.
pub(crate) const DIRECTIONS: [&str; 8] = ["East", "SouthEast", "South", "SouthWest", "West", "NorthWest", "North", "NorthEast"];
/// OP2Lua's difficulty names by number; a multiplayer lobby's resource level is a difficulty (0, "Easy", is High).
pub(crate) const LEVEL_NAMES: [&str; 3] = ["Easy", "Normal", "Hard"];
/// op2-missionhost's type codes of the beacon kinds: mining beacon, magma vent, fumarole.
const BEACON_TYPES: [&str; 3] = ["BEACON", "MAGMA_VENT", "FUMAROLE"];
/// Initial vehicles in the runs that find what the lobby's Initial Vehicles makes (the most the lobby offers).
const VEHICLES: i32 = 12;
/// A beacon spot and the ways the runs saw it made.
type Ways = ((i32, i32), BTreeSet<Beacon>);
/// Beacon spots of which every game has `count`, and the kinds made on them when every game makes the same.
type Picked = (usize, Vec<Ways>, Option<Vec<Beacon>>);
/// A spread layout seen at this many start tiles or more, filling at least 1/START_FILL of the rectangle around them,
/// starts anywhere in that rectangle (Flood Plain rolls a tile in a 32 x 32 square of each quarter).
const START_AREA_MIN: usize = 20;
/// See [`START_AREA_MIN`]: the start tiles seen must be at least 1/START_FILL of their rectangle's tiles.
const START_FILL: usize = 10;
/// More beacon tiles than this that no pattern explains are noted as one line, not one each.
const MANY_BEACONS: usize = 20;
/// A player who owns nothing after the setup.
const NO_START: (i32, i32) = (i32::MIN, i32::MIN);
/// Command Centers this close (tiles, either axis) belong to the same area.
const AREA_GAP: i32 = 16;
/// An area's layouts of one shape, at least this many, are one kit on a rolled start tile ("spread").
const SPREAD: usize = 3;
/// Games run to find every way a column start can turn: until this many in a row turn up no new way, at most
/// COLUMN_GAMES, with seeds from COLUMN_SEEDS on (past the base runs').
const COLUMN_QUIET: u32 = 32;
/// The most games run to find a column start's ways (see [`COLUMN_QUIET`]).
const COLUMN_GAMES: u32 = 256;
/// The first seed index of those games, past the base runs' (see [`COLUMN_QUIET`]).
const COLUMN_SEEDS: u32 = 1000;

/// The converted mission.
pub struct Lua {
    /// File base name: `<Base>.dll`, `<Base>.lua`, `<Base>.placement.lua`.
    pub base: String,
    /// The menu name (the original's, plus " (Lua)").
    pub name: String,
    /// The text of `<Base>.placement.lua`: what every run makes.
    pub placement: String,
    /// The text of `<Base>.lua`: the rest of the setup, the win, the timed functions and the notes.
    pub mission: String,
    /// What wasn't converted, also at the end of `mission`.
    pub notes: Vec<String>,
    /// Computer players the stub asks the game for (`DescBlockEx`): 1 when the mission's is converted.
    pub ai_players: i32,
}

/// The lobby's choices for one run.
#[derive(Clone, Copy)]
struct Lobby {
    /// Every player Eden, else Plymouth.
    eden: bool,
    /// The resource level, the same for every player: 0 High to 2 Low.
    level: i32,
    /// Day and Night on.
    day_night: bool,
    /// Morale Steady on.
    steady: bool,
    /// Initial Vehicles: how many the lobby gives each player.
    vehicles: i32,
    /// Disasters on.
    disasters: bool,
}

/// The lobby of the base runs; the other runs change a choice or two of it.
const BASE_LOBBY: Lobby = Lobby { eden: true, level: 0, day_night: true, steady: true, vehicles: 0, disasters: true };

/// What the converter keeps of one game: a whole Run (the map's tiles, the full call log) for every one of
/// hundreds of games doesn't fit in memory on the big maps.
struct Game {
    /// The player count it was started with.
    players: u8,
    /// Each player's units, from `created`.
    units: BTreeMap<i32, Vec<Unit>>,
    /// The walls, tubes and other tile edits it made, by tile (map file tiles) and type code.
    walls: BTreeSet<((i32, i32), String)>,
    /// The beacons, vents and fumaroles it made, by tile (map file tiles).
    beacons: BTreeMap<(i32, i32), Beacon>,
    /// Each player's vehicles and structures on the map after the setup.
    on_map: BTreeMap<i32, usize>,
}

impl Game {
    /// What's kept of run `r`, whose units `created` has already read.
    fn of(r: &Run, units: BTreeMap<i32, Vec<Unit>>) -> Game {
        let mut on_map = BTreeMap::new();
        for o in r.setup.objects.iter().filter(|o| matches!(o.kind, crate::setup::Kind::Vehicle | crate::setup::Kind::Structure)) {
            *on_map.entry(o.owner).or_insert(0) += 1;
        }
        Game { players: r.players, units, walls: walls_of(&r.setup), beacons: beacons_of(&r.setup), on_map }
    }
}

/// One run of the mission's setup: the player count, what it made and its calls.
pub(crate) struct Run {
    pub(crate) players: u8,
    /// What the setup made: objects, walls, the players' starts.
    pub(crate) setup: Setup,
    /// The call log of the run, every phase.
    pub(crate) calls: Vec<CallRec>,
}

/// The setup of a network game of `players` from `seed` and the `lobby`'s choices, or why it stopped.
fn run(dll: &Path, players: u8, seed: u32, lobby: Lobby) -> Result<Run> {
    let start = NetStart {
        seed,
        slots: (0..players as i32).map(|i| NetSlot { eden: lobby.eden, color: i, resources: lobby.level }).collect(),
        day_night: lobby.day_night,
        disasters: lobby.disasters,
        morale_steady: lobby.steady,
        initial_vehicles: lobby.vehicles,
        lift_limits: false,
        classic_rules: true,
    };
    let opts = start.options(Options { call_log: true, ..Options::from_env() });
    let (setup, calls) = crate::setup::run_with(dll, &opts)?;
    if let Some(f) = &setup.failed {
        bail!("the setup stopped: {f}");
    }
    Ok(Run { players, setup, calls })
}

/// Seeds spread over the whole range: op2-missionhost's first rolls follow the seed's low bits, so consecutive seeds
/// don't reach every value.
pub(crate) fn seed(i: u32) -> u32 {
    1u32.wrapping_add(i.wrapping_mul(2_654_435_761))
}

/// One unit a setup asked for (`TethysGame::CreateUnit`), in map file tiles.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct Unit {
    /// Structures sort first: a base is built, then its vehicles arrive.
    pub(crate) vehicle: bool,
    /// The type's map id.
    pub(crate) type_id: i32,
    pub(crate) tile: (i32, i32),
    /// The way it faces, in eighths of a turn from East (0-7, [`DIRECTIONS`]).
    pub(crate) rotation: i32,
    /// The weapon's map id, or a ConVec's kit (the structure it carries); 0 for none.
    pub(crate) weapon: i32,
    /// A Cargo Truck's load (the SDK's `TruckCargo`, 0 empty) and amount.
    pub(crate) cargo: (i32, i32),
    /// A ConVec's kit's weapon (a Guard Post kit's, `Unit::SetCargo`), 0 for none.
    pub(crate) topping: i32,
    /// A vehicle's headlights after the setup: `CreateUnit` makes them off. `created` leaves them off; colony.rs
    /// reads them off the map.
    pub(crate) lights: bool,
    /// A vehicle the setup parks in a Garage right after making it (`Unit::PutInGarage`): the bay (0-5) and the
    /// Garage's dock tile, in map file tiles.
    pub(crate) garage: Option<(i32, (i32, i32))>,
}

impl Unit {
    /// The same unit with no weapon, for matching Eden's and Plymouth's.
    fn unarmed(&self) -> Unit {
        Unit { weapon: 0, ..self.clone() }
    }
}

/// The map id of op2-missionhost's type code (`CON_TRUCK`), 0 for one it doesn't know.
pub(crate) fn map_id(code: &str) -> i32 {
    MAP_IDS.iter().position(|c| *c == code).map_or(0, |i| i as i32)
}

/// A call argument as a number.
pub(crate) fn int(s: &str) -> Option<i32> {
    s.trim().parse().ok()
}

/// Each player's units, in the order the setup made them, from its `CreateUnit` calls. A Cargo Truck's load and a
/// ConVec's kit are read from the units on the map after the setup, matched to the calls by owner and type in the
/// order they were made (the call log names a new unit only in the calls after the one that made it). A
/// `Unit::PutInGarage` call parks the unit made last; `pad` is the map's x offset.
pub(crate) fn created(r: &Run, pad: i32) -> BTreeMap<i32, Vec<Unit>> {
    let mut made: Vec<&crate::setup::Object> =
        r.setup.objects.iter().filter(|o| matches!(o.kind, crate::setup::Kind::Vehicle | crate::setup::Kind::Structure)).collect();
    made.sort_by_key(|o| o.id);
    // By owner and type: each unit's truck load and amount, and its kit and the kit's weapon.
    type Loads = std::collections::VecDeque<((i32, i32), (i32, i32))>;
    let mut loads: BTreeMap<(i32, i32), Loads> = BTreeMap::new();
    for o in made {
        let e = |k: &str| o.extra.get(k).copied().unwrap_or(0) as i32;
        let kit = (o.weapon_id.max(0), e("cargoWeapon"));
        loads.entry((o.owner, o.type_id)).or_default().push_back(((e("truckCargo"), e("truckAmount")), kit));
    }
    let mut out: BTreeMap<i32, Vec<Unit>> = BTreeMap::new();
    let mut last: Option<i32> = None;
    for c in r.calls.iter().filter(|c| c.phase == "InitProc") {
        // "this=Unit#n, bay, x, y", for the unit just made.
        if c.api == "Unit::PutInGarage" {
            if let [_, bay, x, y] = c.args.iter().flat_map(|a| a.split(',')).map(str::trim).collect::<Vec<_>>()[..]
                && let (Some(bay), Some(x), Some(y)) = (int(bay), int(x), int(y))
                && let Some(u) = last.and_then(|o| out.get_mut(&o)).and_then(|l| l.last_mut())
            {
                u.garage = Some((bay, (x - pad, y)));
            }
            continue;
        }
        if c.api != "TethysGame::CreateUnit" {
            continue;
        }
        // "Unit#n, TYPE, (x,y), owner, WEAPON|None, rotation"
        let [_, kind, x, y, owner, weapon, rotation] = c.args.iter().flat_map(|a| a.split(',')).map(str::trim).collect::<Vec<_>>()[..] else {
            continue;
        };
        let (Some(x), Some(y), Some(owner)) = (int(x.trim_start_matches('(')), int(y.trim_end_matches(')')), int(owner)) else { continue };
        let type_id = map_id(kind);
        let (load, kit) = loads.get_mut(&(owner, type_id)).and_then(|l| l.pop_front()).unwrap_or(((0, 0), (0, 0)));
        // A ConVec the setup loads after making it (`Unit::SetCargo`: Eden 7's and Plymouth 7's Guard Post kits) has
        // its kit and the kit's weapon on the map.
        let convec = type_id == map_id("CON_TRUCK");
        let weapon = if weapon != "None" { map_id(weapon) } else if convec { kit.0 } else { 0 };
        out.entry(owner).or_default().push(Unit {
            vehicle: !is_building(type_id),
            type_id,
            tile: (x - pad, y),
            rotation: int(rotation).unwrap_or(0).rem_euclid(8),
            weapon,
            cargo: if load.0 > 0 { load } else { (0, 0) },
            topping: if convec && weapon == kit.0 { kit.1.max(0) } else { 0 },
            lights: false,
            garage: None,
        });
        last = Some(owner);
    }
    out
}

/// Where lava may flow after the setup (`GameMap::SetLavaPossible`, the last call for a tile wins), as status-bar
/// rectangles `[x1, y1, x2, y2]`: each row's runs of marked tiles, a run joined to the one below it when they span
/// the same columns. The maps mark no tile themselves, so a call that clears a mark changes nothing. Tiles off the map
/// (`size`, in tiles) are left out: Olympus Mons marks a few in the columns left of its first.
pub(crate) fn lava_rects(calls: &[CallRec], pad: i32, size: (i32, i32)) -> Vec<[i32; 4]> {
    let mut marked: BTreeMap<(i32, i32), bool> = BTreeMap::new();
    for c in calls.iter().filter(|c| c.api == "GameMap::SetLavaPossible" && c.phase == "InitProc") {
        // "(x,y), on"
        let [x, y, on] = c.args.iter().flat_map(|a| a.split(',')).map(str::trim).collect::<Vec<_>>()[..] else { continue };
        if let (Some(x), Some(y), Some(on)) = (int(x.trim_start_matches('(')), int(y.trim_end_matches(')')), int(on))
            && (0..size.0).contains(&(x - pad))
            && (0..size.1).contains(&y)
        {
            marked.insert((y, x - pad), on != 0);
        }
    }
    let mut runs: BTreeMap<i32, Vec<(i32, i32)>> = BTreeMap::new();
    for (&(y, x), _) in marked.iter().filter(|(_, on)| **on) {
        let row = runs.entry(y).or_default();
        match row.last_mut() {
            Some(r) if r.1 + 1 == x => r.1 = x,
            _ => row.push((x, x)),
        }
    }
    // Open rectangles by their columns, with the rows they start and end on.
    let mut open: BTreeMap<(i32, i32), (i32, i32)> = BTreeMap::new();
    let mut done: Vec<[i32; 4]> = Vec::new();
    for (&y, row) in &runs {
        let mut next = BTreeMap::new();
        for &cols in row {
            let top = match open.remove(&cols) {
                Some((top, last)) if last + 1 == y => top,
                Some((top, last)) => {
                    done.push([cols.0, top, cols.1, last]);
                    y
                }
                None => y,
            };
            next.insert(cols, (top, y));
        }
        done.extend(open.into_iter().map(|(cols, (top, last))| [cols.0, top, cols.1, last]));
        open = next;
    }
    done.extend(open.into_iter().map(|(cols, (top, last))| [cols.0, top, cols.1, last]));
    let mut rects: Vec<[i32; 4]> = done.into_iter().map(|[x1, y1, x2, y2]| [x1 + 1, y1 + 1, x2 + 1, y2 + 1]).collect();
    rects.sort_by_key(|r| (r[1], r[0]));
    rects
}

/// A player's start: where its Command Center was made, else its first structure, else its first unit.
fn start_of(units: &[Unit]) -> Option<(i32, i32)> {
    let cc = map_id("COMMAND");
    units.iter().find(|u| u.type_id == cc).or_else(|| units.iter().find(|u| !u.vehicle)).or_else(|| units.first()).map(|u| u.tile)
}

/// OP2Lua's type name of a map id, else op2-missionhost's.
pub(crate) fn lua_name(id: i32) -> String {
    match usize::try_from(id).ok().and_then(|i| LUA_NAMES.get(i)) {
        Some(n) if *n != "None" => n.to_string(),
        _ => op2_missionhost::names::map_id_name(id),
    }
}

/// Status-bar coordinates of a map file tile.
pub(crate) fn at((x, y): (i32, i32)) -> String {
    format!("{{ {}, {} }}", x + 1, y + 1)
}

/// A beacon's variant as OP2Lua names it (Tethys's OreVariant), from the game's yield (`bar`: 0 three bars, 1 two,
/// 2 one, -1 random) and variant number: the game numbers a 1-bar beacon's High, Mid, Low 0, 1, 2 and a 2- or 3-bar
/// one's Mid, Low, High (MineManager::GetVariantNum; Plymouth 12's Bar3 beacons of variant 0 are Mid).
pub(crate) fn variant_name(bar: i64, variant: i64) -> &'static str {
    match (bar, variant) {
        (2, 0) => "High",
        (2, 1) => "Mid",
        (2, 2) => "Low",
        (0 | 1, 0) => "Mid",
        (0 | 1, 1) => "Low",
        (0 | 1, 2) => "High",
        // A random yield with a set variant: the variant's meaning waits on the yield drawn.
        (_, 0) => "High",
        (_, 1) => "Mid",
        (_, 2) => "Low",
        _ => "Random",
    }
}

/// A Lua string literal: backslashes, quotes and line breaks escaped (the demos' victory text has line breaks).
pub(crate) fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r"))
}

/// A player's calls of one `_Player` function in a run: the arguments after `this=PlayerN`.
pub(crate) fn player_calls<'a>(calls: &'a [CallRec], api: &'a str, player: i32) -> impl Iterator<Item = &'a [String]> + 'a {
    let this = format!("this=Player{player}");
    calls.iter().filter(move |c| c.api == api && c.args.first() == Some(&this)).map(|c| &c.args[1..])
}

/// A beacon as the mission asked for it: type, ore (-1 random), yield (-1 random), variant (-1 random).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Beacon {
    /// One of [`BEACON_TYPES`].
    pub(crate) type_name: String,
    pub(crate) ore: i64,
    /// The yield as the game numbers it: 0 three bars, 1 two, 2 one (see [`variant_name`]).
    pub(crate) bar: i64,
    pub(crate) variant: i64,
}

/// The beacons, magma vents and fumaroles on the map after the setup, by map file tile.
fn beacons_of(s: &Setup) -> BTreeMap<(i32, i32), Beacon> {
    s.objects
        .iter()
        .filter(|o| BEACON_TYPES.contains(&o.type_name.as_str()))
        .map(|o| {
            let e = |k: &str| o.extra.get(k).copied().unwrap_or(-1);
            (o.tile, Beacon { type_name: o.type_name.clone(), ore: e("oreType"), bar: e("yield"), variant: e("variant") })
        })
        .collect()
}

/// The setup's walls, tubes and other tile edits, by map file tile and type code.
fn walls_of(s: &Setup) -> BTreeSet<((i32, i32), String)> {
    s.walls.iter().map(|w| (w.tile, w.type_name.clone())).collect()
}

/// An area's name: its compass point from the map's centre.
fn compass(tile: (i32, i32), size: (i32, i32)) -> String {
    let (dx, dy) = (tile.0 - size.0 / 2, tile.1 - size.1 / 2);
    let ns = if dy * 6 < -size.1 { "N" } else if dy * 6 > size.1 { "S" } else { "" };
    let ew = if dx * 6 < -size.0 { "W" } else if dx * 6 > size.0 { "E" } else { "" };
    let name = format!("{ns}{ew}");
    if name.is_empty() { "C".into() } else { name }
}

/// What a resource level gives a player.
#[derive(Clone, PartialEq, Default)]
pub(crate) struct Level {
    /// Colonists and stores by OP2Lua's field name (`workers`, `food`, `rare_ore`...).
    pub(crate) numbers: Vec<(&'static str, i32)>,
    /// The topics marked researched (`_Player::MarkResearchComplete`), in call order.
    pub(crate) research: Vec<i32>,
    /// The last `_Player::SetTechLevel`, if any.
    pub(crate) tech_level: Option<i32>,
    /// The Structure Factory's stored kits: the bay (1-6), the kit's map id and its weapon's (0 for none).
    pub(crate) kits: Vec<(usize, i32, i32)>,
}

/// What `player` started with in run `r`: its colonists and stores after the setup, its research calls and its
/// Structure Factory's bays.
pub(crate) fn level_of(r: &Run, player: i32) -> Level {
    let s = r.setup.players.iter().find(|p| p.id == player).map(|p| p.start.clone()).unwrap_or_default();
    let numbers = vec![
        ("workers", s.workers),
        ("scientists", s.scientists),
        ("kids", s.kids),
        ("food", s.food),
        ("ore", s.ore),
        ("rare_ore", s.rare_ore),
    ];
    let research = player_calls(&r.calls, "_Player::MarkResearchComplete", player).filter_map(|a| int(a.first()?)).collect();
    let tech_level = player_calls(&r.calls, "_Player::SetTechLevel", player).filter_map(|a| int(a.first()?)).last();
    let factory = r.setup.objects.iter().find(|o| o.owner == player && o.type_name == "FACT_STRUCTURE");
    let kits = factory
        .map(|f| {
            (0..6)
                .filter_map(|bay| {
                    // A bay holds the kit's map id in its high 16 bits and the kit's weapon in its low 16; 0 is empty.
                    let v = *f.extra.get(&format!("bay{bay}"))?;
                    ((v >> 16) > 0).then_some((bay + 1, (v >> 16) as i32, (v & 0xFFFF) as i32))
                })
                .collect()
        })
        .unwrap_or_default();
    Level { numbers, research, tech_level, kits }
}

/// The morale calls of a run, as OP2Lua calls.
pub(crate) fn morale_calls(calls: &[CallRec]) -> Vec<String> {
    calls
        .iter()
        .filter_map(|c| {
            let level = match c.api.as_str() {
                "TethysGame::ForceMoraleGreat" => "Excellent",
                "TethysGame::ForceMoraleGood" => "Good",
                "TethysGame::ForceMoraleOK" => "Fair",
                "TethysGame::ForceMoralePoor" => "Poor",
                "TethysGame::ForceMoraleRotten" => "Terrible",
                "TethysGame::FreeMoraleLevel" => return Some("game.free_morale()".to_string()),
                _ => return None,
            };
            Some(format!("game.morale_steady({})", quoted(level)))
        })
        .collect()
}

/// The last argument, as a number, of the last call of `api` that has a numeric one.
pub(crate) fn last_arg(calls: &[CallRec], api: &str) -> Option<i32> {
    calls.iter().filter(|c| c.api == api).filter_map(|c| int(c.args.last()?)).next_back()
}

/// One base layout: its units, which resource levels get each (`[Easy, Normal, Hard]`), what only it brings, and
/// where the lobby's initial vehicles are dropped.
#[derive(Default, Clone)]
struct Variant {
    units: Vec<(Unit, [bool; 3])>,
    /// The tile the player's view starts centred on, from the setup's player start (map file tiles).
    view: Option<(i32, i32)>,
    /// Where the lobby's initial vehicles are dropped (map file tiles).
    spots: BTreeSet<(i32, i32)>,
    /// Runs (of `base_runs`) that used it.
    runs: Vec<bool>,
}

/// Beacon spot groups (each spot with whether each game had it) moved spot by spot between groups until each group's
/// count is the same in every game, or no move helps: a mission's spot lists needn't keep to the nearest area (Flood
/// Plain's two southern lists reach across the middle of the map). Groups that end up empty are dropped.
fn settle_groups(mut groups: Vec<Vec<&(Ways, Vec<bool>)>>, runs: usize) -> Vec<Vec<&(Ways, Vec<bool>)>> {
    let counts = |g: &[&(Ways, Vec<bool>)]| -> Vec<i64> { (0..runs).map(|i| g.iter().filter(|(_, p)| p[i]).count() as i64).collect() };
    // How far a group's counts are from one number: the sum of squares around their mean, times the run count.
    let spread = |c: &[i64]| -> i64 {
        let (n, sum) = (c.len() as i64, c.iter().sum::<i64>());
        c.iter().map(|&x| (x * n - sum).pow(2)).sum::<i64>() / n.max(1)
    };
    for _ in 0..1000 {
        let mut best: Option<(i64, usize, usize, usize)> = None;
        let cost: Vec<i64> = groups.iter().map(|g| spread(&counts(g))).collect();
        for from in 0..groups.len() {
            for b in 0..groups[from].len() {
                let mut without = groups[from].clone();
                let moved = without.remove(b);
                let from_cost = spread(&counts(&without));
                for to in (0..groups.len()).filter(|&t| t != from) {
                    let mut with = groups[to].clone();
                    with.push(moved);
                    let delta = from_cost + spread(&counts(&with)) - cost[from] - cost[to];
                    if delta < 0 && best.is_none_or(|(d, ..)| delta < d) {
                        best = Some((delta, from, b, to));
                    }
                }
            }
        }
        let Some((_, from, b, to)) = best else { break };
        let moved = groups[from].remove(b);
        groups[to].push(moved);
    }
    groups.retain(|g| !g.is_empty());
    for g in &mut groups {
        g.sort_by_key(|(w, _)| w.0);
    }
    groups
}

/// Units moved by `by` tiles.
fn shifted(units: &[Unit], by: (i32, i32)) -> Vec<Unit> {
    units.iter().map(|u| Unit { tile: (u.tile.0 + by.0, u.tile.1 + by.1), ..u.clone() }).collect()
}

/// Tile `t` moved by `by` tiles.
fn offset(t: (i32, i32), by: (i32, i32)) -> (i32, i32) {
    (t.0 + by.0, t.1 + by.1)
}

/// Each column start's arrangements, by the way they face.
type Columns = BTreeMap<(i32, i32), BTreeMap<i32, Vec<Unit>>>;
/// The arrangements of units seen at one start, by the way the first unit faces.
type Ways8 = BTreeMap<i32, BTreeSet<Vec<Unit>>>;
/// Tiles the lobby's initial vehicles are dropped on.
type Spots = BTreeSet<(i32, i32)>;

/// A game's units at start `t`, turned to the way its layout was first seen when `t` is a column start: the column in
/// that arrangement (with this game's weapons), then whatever came after it (the lobby's initial vehicles). None if
/// they don't fit one of the column's arrangements.
fn as_first(columns: &Columns, variants: &BTreeMap<(i32, i32), Variant>, t: (i32, i32), us: &[Unit]) -> Option<Vec<Unit>> {
    let Some(forms) = columns.get(&t) else { return Some(us.to_vec()) };
    let this = forms.get(&us.first()?.rotation)?;
    let first = &variants.get(&t)?.units;
    if us.len() < this.len() || first.len() != this.len() || this.iter().zip(us).any(|(a, b)| a.unarmed() != b.unarmed()) {
        return None;
    }
    let mut out: Vec<Unit> = first.iter().zip(us).map(|((f, _), u)| Unit { weapon: u.weapon, ..f.clone() }).collect();
    out.extend(us[this.len()..].iter().cloned());
    Some(out)
}

/// Units in `with` that `base` hasn't, as a multiset difference.
fn minus(with: &[Unit], base: &[Unit]) -> Vec<Unit> {
    let mut extra = with.to_vec();
    for u in base {
        if let Some(i) = extra.iter().position(|e| e == u) {
            extra.remove(i);
        }
    }
    extra
}

/// Converts `dll` into an OP2Lua mission: a single-player one through `colony::convert`, a multiplayer one here.
/// `progress` hears about the runs. `game_dir` finds the map, whose width decides the coordinates (no 32-tile pad on
/// a 512-wide map). With `simple`, each area gets one base, the same every game, and the beacons are fixed
/// (`<dll>Simple`); without it, and without a computer player, the setup is a replay of the original's.
pub fn convert(dll: &Path, game_dir: Option<&Path>, simple: bool, progress: &mut dyn FnMut(&str)) -> Result<Lua> {
    let report = crate::inspect::inspect(dll, game_dir)?;
    let desc = report.desc.as_ref().with_context(|| format!("{} isn't a mission", dll.display()))?;
    // Colony games, auto demos, tutorials and campaign missions (positive types): no lobby, the difficulty chosen in
    // the menu.
    if matches!(desc.mission_type, -3..=-1) || desc.mission_type > 0 {
        return crate::colony::convert(dll, &report, progress);
    }
    if !is_multiplayer(desc.mission_type) {
        bail!("{} is a {} mission; `lua` converts multiplayer missions, campaign missions, colony games, auto demos and tutorials only for now", dll.display(), crate::inspect::mission_kind(desc.mission_type));
    }
    let max = desc.num_players.clamp(2, 6) as u8;
    let size = report.map.as_ref().map(|m| (m.width as i32, m.height as i32)).with_context(|| format!("{}: the map wasn't found (--game-dir)", dll.display()))?;
    // The game's x is the file's plus 32 on a map that doesn't wrap (narrower than 512 tiles).
    let pad = if size.0 >= 512 { 0 } else { 32 };
    let mut notes: Vec<String> = Vec::new();
    let note = |notes: &mut Vec<String>, n: String| {
        if !notes.contains(&n) {
            notes.push(n);
        }
    };

    // The base runs: every player count, seeds until no new layout or hand-out turns up.
    let mut base_runs: Vec<Game> = Vec::new();
    let mut variants: BTreeMap<(i32, i32), Variant> = BTreeMap::new();
    let mut handouts: BTreeMap<u8, BTreeSet<Vec<(i32, i32)>>> = BTreeMap::new();
    let mut extra_runs: Vec<(Lobby, BTreeMap<i32, Vec<Unit>>)> = Vec::new();
    // Each layout's units relative to its start tile, and the first layout seen of each such shape: a Land Rush
    // map puts the same kit on a start tile rolled from a list, so its many layouts are one shape.
    let mut shape_of: BTreeMap<(i32, i32), Vec<Unit>> = BTreeMap::new();
    let mut first_of_shape: BTreeMap<Vec<Unit>, (i32, i32)> = BTreeMap::new();
    // Each structure type's footprint (width, height), from the runs.
    let mut sizes: BTreeMap<i32, (i32, i32)> = BTreeMap::new();
    // Every arrangement of units seen at each start, by the way its first unit faces.
    let mut arrangements: BTreeMap<(i32, i32), Ways8> = BTreeMap::new();
    for n in (2..=max).rev() {
        let (mut quiet, mut i) = (0, 0);
        // At least `least` seeds, then until `enough` in a row bring nothing new, at most `most`; more of each at the
        // most players.
        let (least, enough, most) = if n == max { (24, 20, 128) } else { (16, 12, 64) };
        while i < most && (i < least || quiet < enough) {
            let s = seed(i);
            let r = run(dll, n, s, BASE_LOBBY)?;
            i += 1;
            for o in r.setup.objects.iter().filter(|o| matches!(o.kind, crate::setup::Kind::Structure)) {
                let ((x0, y0), (x1, y1)) = o.footprint;
                sizes.insert(o.type_id, (x1 - x0 + 1, y1 - y0 + 1));
            }
            let units = created(&r, pad);
            let starts: Vec<(i32, i32)> = (0..n as i32).map(|p| units.get(&p).and_then(|u| start_of(u)).unwrap_or(NO_START)).collect();
            for (p, t) in starts.iter().enumerate() {
                if let Some(us) = units.get(&(p as i32)).filter(|us| !us.is_empty()) {
                    arrangements.entry(*t).or_default().entry(us[0].rotation).or_default().insert(us.clone());
                }
            }
            let mut new = handouts.entry(n).or_default().insert(starts.clone());
            for (p, t) in starts.iter().enumerate() {
                if *t == NO_START || variants.contains_key(t) {
                    continue;
                }
                new = true;
                let view = r.setup.players.iter().find(|x| x.id == p as i32).and_then(|x| x.start.view);
                let us = units.get(&(p as i32)).cloned().unwrap_or_default();
                let shape = shifted(&us, (-t.0, -t.1));
                let new_shape = !first_of_shape.contains_key(&shape);
                if new_shape && n == max {
                    first_of_shape.insert(shape.clone(), *t);
                }
                shape_of.insert(*t, shape);
                variants.insert(*t, Variant { units: us.into_iter().map(|u| (u, [true, false, false])).collect(), view, ..Default::default() });
                // A new layout: run the same game with the lobby's other choices, to see what they change in it
                // (once per shape: the same kit elsewhere changes the same way).
                if n == max && new_shape {
                    for lobby in [
                        Lobby { eden: false, ..BASE_LOBBY },
                        Lobby { level: 1, ..BASE_LOBBY },
                        Lobby { level: 2, ..BASE_LOBBY },
                        Lobby { vehicles: VEHICLES, ..BASE_LOBBY },
                        Lobby { eden: false, vehicles: VEHICLES, ..BASE_LOBBY },
                    ] {
                        let u = created(&run(dll, n, s, lobby)?, pad);
                        extra_runs.push((lobby, u));
                    }
                }
            }
            quiet = if new { 0 } else { quiet + 1 };
            base_runs.push(Game::of(&r, units));
        }
        progress(&format!("{n} players: {i} games, {} layouts so far", variants.len()));
    }
    if variants.is_empty() {
        bail!("no player owns anything after the setup");
    }
    // Columns: a start whose units come out in several arrangements, one per way they face, each the same units in
    // the same order (Land Rush: `GetRand(8)` rolls the way a player's column of vehicles runs from the Robo-Surveyor).
    // More games, with the lobby's most initial vehicles, until no new way turns up and every way has its spots for
    // them (the same Land Rush setups drop each on one of 4 spots, `GetRand(4)`, that turn with the column).
    let is_column = |by: &Ways8| -> bool {
        let all: Vec<&Vec<Unit>> = by.values().flatten().collect();
        let kinds = |us: &Vec<Unit>| -> Vec<(i32, i32, (i32, i32))> { us.iter().map(|u| (u.type_id, u.weapon, u.cargo)).collect() };
        all.len() > 1
            && by.values().all(|s| s.len() == 1)
            && all.iter().all(|us| kinds(us) == kinds(all[0]) && us.iter().all(|u| !u.vehicle || u.rotation == us[0].rotation))
    };
    let mut column_spots: BTreeMap<((i32, i32), i32), Spots> = BTreeMap::new();
    if arrangements.values().any(is_column) {
        let (mut quiet, mut i) = (0, 0);
        while quiet < COLUMN_QUIET && i < COLUMN_GAMES {
            let r = run(dll, max, seed(COLUMN_SEEDS + i), Lobby { vehicles: VEHICLES, ..BASE_LOBBY })?;
            i += 1;
            let mut new = false;
            for us in created(&r, pad).into_values() {
                let Some(t) = start_of(&us) else { continue };
                let Some(by) = arrangements.get_mut(&t).filter(|by| is_column(by)) else { continue };
                let len = by.values().flatten().next().map_or(0, Vec::len);
                if us.len() < len {
                    continue;
                }
                let way = us[0].rotation;
                let spots = column_spots.entry((t, way)).or_default();
                new |= !by.contains_key(&way) || spots.is_empty();
                spots.extend(us[len..].iter().map(|u| u.tile));
                by.entry(way).or_default().insert(us[..len].to_vec());
            }
            quiet = if new { 0 } else { quiet + 1 };
        }
        progress(&format!("columns: {i} more games"));
    }
    let columns: Columns = arrangements
        .iter()
        .filter(|(t, by)| variants.contains_key(t) && is_column(by))
        .map(|(t, by)| (*t, by.iter().filter_map(|(f, s)| Some((*f, s.iter().next()?.clone()))).collect()))
        .collect();
    // The resource level changes the mission's rolls (Peacekeeper's seed 3 gives other layouts at Medium than at
    // High), so a layout's own seed may not give it at the other levels: more seeds at each level, until every
    // shape's first layout has turned up there.
    for level in 1..3 {
        let lobby = Lobby { level, ..BASE_LOBBY };
        let mut missing: BTreeSet<(i32, i32)> = first_of_shape
            .values()
            .filter(|t| !extra_runs.iter().any(|(l, u)| l.level == level && u.values().any(|us| start_of(us) == Some(**t))))
            .copied()
            .collect();
        let mut i = 0;
        while !missing.is_empty() && i < 128 {
            let u = created(&run(dll, max, seed(i), lobby)?, pad);
            i += 1;
            let mut hit = false;
            for us in u.values() {
                if let Some(t) = start_of(us) {
                    hit |= missing.remove(&t);
                }
            }
            if hit {
                extra_runs.push((lobby, u));
            }
        }
        for t in missing {
            note(&mut notes, format!("the layout at {} never came up at resource level {} (High's units are used)", at(t), LEVEL_NAMES[level as usize]));
        }
    }
    for (_, units) in extra_runs.iter_mut() {
        for us in units.values_mut() {
            if let Some(t) = start_of(us) {
                *us = as_first(&columns, &variants, t, us).unwrap_or_default();
            }
        }
    }
    let base_count = base_runs.len();
    for (i, g) in base_runs.iter().enumerate() {
        for p in 0..g.players as i32 {
            if let Some(v) = g.units.get(&p).and_then(|u| start_of(u)).and_then(|t| variants.get_mut(&t)) {
                v.runs.resize(base_count, false);
                v.runs[i] = true;
            }
        }
    }
    for v in variants.values_mut() {
        v.runs.resize(base_count, false);
    }
    // A layout must always come out the same; a run that disagrees is noted.
    for g in &base_runs {
        for p in 0..g.players as i32 {
            let Some(us) = g.units.get(&p) else { continue };
            let Some(t) = start_of(us) else { continue };
            let Some(v) = variants.get(&t) else { continue };
            let Some(us) = as_first(&columns, &variants, t, us) else {
                note(&mut notes, format!("the layout at {} isn't always the same (its units vary)", at(t)));
                continue;
            };
            let known: Vec<&Unit> = v.units.iter().map(|(u, _)| u).collect();
            if known.len() != us.len() || known.iter().zip(&us).any(|(a, b)| *a != b) {
                note(&mut notes, format!("the layout at {} isn't always the same (its units vary)", at(t)));
            }
        }
    }

    // Areas: layouts whose Command Centers are close together, named by compass point.
    let tiles: Vec<(i32, i32)> = variants.keys().copied().collect();
    let mut area_of: BTreeMap<(i32, i32), usize> = BTreeMap::new();
    let mut areas: Vec<Vec<(i32, i32)>> = Vec::new();
    for t in &tiles {
        let near: BTreeSet<usize> = areas
            .iter()
            .enumerate()
            .filter(|(_, a)| a.iter().any(|o| (o.0 - t.0).abs() <= AREA_GAP && (o.1 - t.1).abs() <= AREA_GAP))
            .map(|(i, _)| i)
            .collect();
        match near.iter().next() {
            None => areas.push(vec![*t]),
            Some(&first) => {
                // Joins (and merges) every area it touches.
                for &other in near.iter().skip(1).rev() {
                    let moved = areas.remove(other);
                    areas[first].extend(moved);
                }
                areas[first].push(*t);
            }
        }
    }
    let mut area_names: Vec<String> = Vec::new();
    for a in &areas {
        let centre = (a.iter().map(|t| t.0).sum::<i32>() / a.len() as i32, a.iter().map(|t| t.1).sum::<i32>() / a.len() as i32);
        let mut name = compass(centre, size);
        if area_names.contains(&name) {
            name = format!("{name}{}", area_names.iter().filter(|n| n.starts_with(&name)).count() + 1);
        }
        area_names.push(name);
    }
    for (i, a) in areas.iter().enumerate() {
        for t in a {
            area_of.insert(*t, i);
        }
    }
    let variant_name = |t: (i32, i32)| -> String {
        let a = area_of[&t];
        let i = areas[a].iter().position(|o| *o == t).unwrap_or(0);
        if areas[a].len() == 1 { area_names[a].clone() } else { format!("{}{}", area_names[a], i + 1) }
    };
    // Per area, whether each base run gave one of its layouts out.
    let area_runs: Vec<Vec<bool>> = areas
        .iter()
        .map(|a| (0..base_count).map(|i| a.iter().any(|t| variants[t].runs[i])).collect())
        .collect();
    // Spread layouts: an area's layouts of one shape (SPREAD or more of them) are one kit on a start tile rolled from
    // theirs, as Land Rush maps place a player's vehicles.
    let spread: Vec<Vec<(i32, i32)>> = {
        /// A layout's area index and its units relative to its start tile.
        type Key<'a> = (usize, &'a Vec<Unit>);
        let mut by: BTreeMap<Key, Vec<(i32, i32)>> = BTreeMap::new();
        for t in variants.keys() {
            if let Some(s) = shape_of.get(t) {
                by.entry((area_of[t], s)).or_default().push(*t);
            }
        }
        by.into_values().filter(|g| g.len() >= SPREAD).collect()
    };
    let spread_of: BTreeMap<(i32, i32), usize> = spread.iter().enumerate().flat_map(|(i, g)| g.iter().map(move |t| (*t, i))).collect();
    let area_centre: Vec<(i32, i32)> = areas
        .iter()
        .map(|a| (a.iter().map(|t| t.0).sum::<i32>() / a.len() as i32, a.iter().map(|t| t.1).sum::<i32>() / a.len() as i32))
        .collect();

    // The lobby's other choices, layout by layout.
    // Plymouth's weapon in place of each of Eden's (map ids), and the lobby's initial vehicle (type, weapon) for Eden
    // and for Plymouth.
    let mut plymouth_weapons: BTreeMap<i32, i32> = BTreeMap::new();
    let mut initial: [Option<(i32, i32)>; 2] = [None, None];
    for (lobby, units) in &extra_runs {
        for (p, us) in units {
            let Some(t) = start_of(us) else { continue };
            let Some(v) = variants.get_mut(&t) else { continue };
            let base: Vec<Unit> = v.units.iter().filter(|(_, l)| l[0]).map(|(u, _)| u.clone()).collect();
            if lobby.level > 0 {
                let l = lobby.level as usize;
                let here = us.clone();
                // Copies of the same unit (three ConVecs on one tile) count one by one.
                let mut left = here.clone();
                for (u, levels) in v.units.iter_mut() {
                    if let Some(i) = left.iter().position(|x| x == u) {
                        left.remove(i);
                        levels[l] = true;
                    }
                }
                for u in minus(&here, &base) {
                    if !v.units.iter().any(|(x, _)| *x == u) {
                        let mut levels = [false; 3];
                        levels[l] = true;
                        v.units.push((u, levels));
                    }
                }
            } else if lobby.vehicles > 0 {
                let extra = minus(us, &if lobby.eden { base.clone() } else { base.iter().map(|u| colony(u, &plymouth_weapons)).collect() });
                if extra.is_empty() {
                    continue;
                }
                let kind = (extra[0].type_id, extra[0].weapon);
                if extra.len() as i32 != lobby.vehicles || extra.iter().any(|e| (e.type_id, e.weapon) != kind) {
                    note(&mut notes, format!("player {}'s initial vehicles don't follow one pattern (not converted)", p + 1));
                    continue;
                }
                initial[usize::from(!lobby.eden)] = Some(kind);
                v.spots.extend(extra.iter().map(|e| e.tile));
            } else if !lobby.eden {
                // Plymouth: the same units, some with other weapons.
                let mut a: Vec<Unit> = base.clone();
                let mut b: Vec<Unit> = us.clone();
                a.sort_by_key(Unit::unarmed);
                b.sort_by_key(Unit::unarmed);
                if a.len() != b.len() || a.iter().zip(&b).any(|(x, y)| x.unarmed() != y.unarmed()) {
                    note(&mut notes, format!("Plymouth gets a different layout at {} (not converted: Eden's is used)", at(t)));
                    continue;
                }
                for (x, y) in a.iter().zip(&b) {
                    if x.weapon != y.weapon {
                        if plymouth_weapons.get(&x.weapon).is_some_and(|w| *w != y.weapon) {
                            note(&mut notes, format!("Plymouth's weapon for Eden's {} isn't always the same", lua_name(x.weapon)));
                        }
                        plymouth_weapons.insert(x.weapon, y.weapon);
                    }
                }
            }
        }
    }
    // Levels a unit wasn't seen at stay off only if that level was run for its layout.
    let level_seen: BTreeSet<((i32, i32), usize)> = extra_runs
        .iter()
        .filter(|(l, _)| l.level > 0)
        .flat_map(|(l, u)| u.values().filter_map(|us| start_of(us)).map(|t| (t, l.level as usize)).collect::<Vec<_>>())
        .collect();
    for (t, v) in variants.iter_mut() {
        for l in 1..3 {
            if !level_seen.contains(&(*t, l)) {
                for (_, levels) in v.units.iter_mut() {
                    levels[l] = levels[0];
                }
            }
        }
    }
    // A layout of a shape seen before had no runs of its own with the lobby's other choices: it gets its shape's
    // first layout's levels and vehicle spots, moved to its start.
    let firsts: BTreeMap<(i32, i32), Variant> =
        first_of_shape.values().filter_map(|t| variants.get(t).map(|v| (*t, v.clone()))).collect();
    for (t, v) in variants.iter_mut() {
        let first = shape_of.get(t).and_then(|s| first_of_shape.get(s)).filter(|f| *f != t).and_then(|f| firsts.get(f).map(|v| (*f, v)));
        if let Some((f, fv)) = first {
            let by = (t.0 - f.0, t.1 - f.1);
            v.units = fv.units.iter().map(|(u, l)| (Unit { tile: offset(u.tile, by), ..u.clone() }, *l)).collect();
            v.spots = fv.spots.iter().map(|s| offset(*s, by)).collect();
        }
    }
    progress(&format!("{} layouts in {} areas; colonies, levels and vehicles compared", variants.len(), areas.len()));
    // The simple version's base in each area: its first layout whose structures all stand on ground a building can
    // take (Peacekeeper's NW3, NW7 and NW8 put one on rock; the game moves it off and the base is cut off).
    // Each area's pick, its layouts with a structure on blocked ground, and for a column start the way it faces: the
    // first (East round to NorthEast) whose units all stand on ground a vehicle can take. When no way does (Axen's
    // Home SW: cracks cross its single start tile's column every way, in the original too), the first way's column
    // is moved by the shift last in the Pick: the nearest one that puts its units and vehicle spots on clear ground
    // with a clear tile all round each unit.
    type Pick = ((i32, i32), Vec<(i32, i32)>, Option<i32>, (i32, i32));
    let chosen: Vec<Vec<Pick>> = if simple {
        let map = op2_formats::GameFiles::open(game_dir.context("the game folder isn't known (--game-dir)")?)
            .and_then(|f| f.read_map(&desc.map_name))
            .with_context(|| format!("reading {}", desc.map_name))?;
        let blocked = |x: i32, y: i32| -> bool {
            use op2_formats::map::CellType::*;
            x < 0
                || y < 0
                || x >= map.width as i32
                || y >= map.height as i32
                || matches!(
                    map.tile(x as u32, y as u32).cell_type,
                    Impassible1 | Impassible2 | NorthCliffs | CliffsHighSide | CliffsLowSide
                )
        };
        // A structure's tile is its footprint's centre, rounded up and left: an even size reaches one tile further
        // right or down (Peacekeeper's NW7 Command Center, 3x2 at 15,6, covers rows 6-7 and was moved off the rock
        // at 16,7 in a game).
        let clear = |t: &(i32, i32)| {
            variants[t].units.iter().filter(|(u, l)| l[0] && !u.vehicle).all(|(u, _)| {
                let (w, h) = sizes.get(&u.type_id).copied().unwrap_or((1, 1));
                let (x0, y0) = (u.tile.0 - (w - 1) / 2, u.tile.1 - (h - 1) / 2);
                (x0..x0 + w).all(|x| (y0..y0 + h).all(|y| !blocked(x, y)))
            })
        };
        let mut chosen = Vec::new();
        for (i, a) in areas.iter().enumerate() {
            let on_rock: Vec<(i32, i32)> = a.iter().filter(|t| !clear(t)).copied().collect();
            let first = a.iter().find(|t| !on_rock.contains(t)).copied().unwrap_or_else(|| {
                note(&mut notes, format!("every layout in area {} has a structure on blocked ground (its first is used)", area_names[i]));
                a[0]
            });
            // An area a hand-out gives to more than one player (Root Canal's N on Land Rush: two start tiles in it)
            // gets as many bases: the starts one of the original's hand-outs gave out together, so they don't overlap,
            // all on clear ground where a hand-out has such.
            let place = |t: &(i32, i32)| a.iter().position(|x| x == t).unwrap_or(usize::MAX);
            let mut together: Vec<Vec<(i32, i32)>> = handouts
                .values()
                .flatten()
                .map(|h| {
                    let mut s: Vec<(i32, i32)> = h.iter().filter(|t| area_of.get(*t) == Some(&i)).copied().collect();
                    s.sort_by_key(place);
                    s.dedup();
                    s
                })
                .collect();
            let most = together.iter().map(Vec::len).max().unwrap_or(1);
            together.retain(|s| s.len() == most);
            together.sort_by_key(|s| s.iter().map(place).collect::<Vec<_>>());
            let tiles = match together.iter().find(|s| s.iter().all(|t| !on_rock.contains(t))).or(together.first()) {
                Some(s) if most > 1 => s.clone(),
                _ => vec![first],
            };
            let mut picks = Vec::new();
            for pick in tiles {
                let mut shift = (0, 0);
                let way = columns.get(&pick).and_then(|forms| {
                    let way = forms.iter().find(|(_, us)| us.iter().all(|u| !blocked(u.tile.0, u.tile.1))).map(|(f, _)| *f);
                    if way.is_some() {
                        return way;
                    }
                    let (&first, us) = forms.iter().next()?;
                    let spots = column_spots.get(&(pick, first)).cloned().unwrap_or_default();
                    let fits = |(dx, dy): (i32, i32)| {
                        us.iter().all(|u| (-1..=1).all(|a| (-1..=1).all(|b| !blocked(u.tile.0 + dx + a, u.tile.1 + dy + b))))
                            && spots.iter().all(|s| !blocked(s.0 + dx, s.1 + dy))
                    };
                    let mut by: Vec<(i32, i32)> = (-16..=16).flat_map(|dy| (-16..=16).map(move |dx| (dx, dy))).collect();
                    by.sort_by_key(|&(dx, dy)| (dx.abs() + dy.abs(), dy, dx));
                    match by.into_iter().find(|s| fits(*s)) {
                        Some(s) => {
                            shift = s;
                            note(&mut notes, format!(
                                "every way the column in area {} turns puts a unit on blocked ground, in the original too; its first way is moved {} east, {} south to clear ground",
                                area_names[i], s.0, s.1
                            ));
                        }
                        None => note(&mut notes, format!("every way the column in area {} turns puts a unit on blocked ground (the first is used)", area_names[i])),
                    }
                    Some(first)
                });
                picks.push((pick, on_rock.clone(), way, shift));
            }
            chosen.push(picks);
        }
        chosen
    } else {
        Vec::new()
    };

    // Tubes, walls and beacons: in every run (placement), with one layout, with one area, or a note.
    let mut wall_runs: BTreeMap<((i32, i32), String), Vec<bool>> = BTreeMap::new();
    let mut beacon_seen: BTreeMap<(i32, i32), (Vec<bool>, BTreeSet<Beacon>)> = BTreeMap::new();
    for (i, g) in base_runs.iter().enumerate() {
        for w in &g.walls {
            wall_runs.entry(w.clone()).or_insert_with(|| vec![false; base_count])[i] = true;
        }
        for (&t, b) in &g.beacons {
            let e = beacon_seen.entry(t).or_insert_with(|| (vec![false; base_count], BTreeSet::new()));
            e.0[i] = true;
            e.1.insert(b.clone());
        }
    }
    /// What brings a tube, wall or beacon.
    #[derive(Clone, Copy, PartialEq)]
    enum Owner {
        /// Every game: placement.lua.
        All,
        /// The layout whose start (Command Center) is at this map file tile.
        Variant((i32, i32)),
        /// The area of this index in `areas`, whichever of its layouts is rolled.
        Area(usize),
    }
    // What brings a tube or beacon: every game, one area, or the layouts whose games are exactly the ones it's in
    // (one layout, or several: a tube two of a corner's layouts share).
    let owner_of = |present: &Vec<bool>| -> Option<Vec<Owner>> {
        if present.iter().all(|&p| p) {
            return Some(vec![Owner::All]);
        }
        if let Some(a) = area_runs.iter().position(|a| a == present) {
            return Some(vec![Owner::Area(a)]);
        }
        let with: Vec<(i32, i32)> = variants
            .iter()
            // A spread layout's own start brings nothing: its tiles are rolled, not each a layout.
            .filter(|(t, _)| !spread_of.contains_key(t))
            .filter(|(_, v)| v.runs.iter().any(|&r| r) && v.runs.iter().zip(present).all(|(&r, &p)| !r || p))
            .map(|(t, _)| *t)
            .collect();
        let covered = (0..present.len()).all(|i| present[i] == with.iter().any(|t| variants[t].runs[i]));
        (covered && !with.is_empty()).then(|| with.into_iter().map(Owner::Variant).collect())
    };
    /// An owner's walls and tubes (tile, type code) and beacons (tile, the ways the runs made it), in map file tiles.
    type Things = (Vec<((i32, i32), String)>, Vec<((i32, i32), BTreeSet<Beacon>)>);
    let mut owned: Vec<(Owner, Things)> = Vec::new();
    let mut add = |owner: Owner, wall: Option<((i32, i32), String)>, beacon: Option<((i32, i32), BTreeSet<Beacon>)>| {
        let i = owned.iter().position(|(o, _)| *o == owner).unwrap_or_else(|| {
            owned.push((owner, (Vec::new(), Vec::new())));
            owned.len() - 1
        });
        owned[i].1.0.extend(wall);
        owned[i].1.1.extend(beacon);
    };
    for (w, present) in &wall_runs {
        match owner_of(present) {
            Some(owners) => owners.into_iter().for_each(|o| add(o, Some(w.clone()), None)),
            None => note(&mut notes, format!("{} at {} appears in some games only, with no one layout", w.1, at(w.0))),
        }
    }
    let mut loose: Vec<(Ways, Vec<bool>)> = Vec::new();
    for (t, (present, kinds)) in &beacon_seen {
        match owner_of(present) {
            Some(owners) => owners.into_iter().for_each(|o| add(o, None, Some((*t, kinds.clone())))),
            None => loose.push(((*t, kinds.clone()), present.clone())),
        }
    }
    // Beacons that come and go with no layout: if every game has the same number of them, the mission picks that
    // many of their spots at random. Tried area by area first (each beacon with its nearest area: Flood Plain picks
    // each quarter's beacons from a list of its own), then all together. When every game also makes the same kinds
    // (Flood Plain: a Common Bar3, a Rare Bar2, the rest random), those kinds go on the picked spots.
    let mut picked: Vec<Picked> = Vec::new();
    let constant = |group: &[&(Ways, Vec<bool>)]| -> Option<usize> {
        let counts: BTreeSet<usize> = (0..base_count).map(|i| group.iter().filter(|(_, p)| p[i]).count()).collect();
        (counts.len() == 1).then(|| counts.into_iter().next()).flatten().filter(|&k| k > 0)
    };
    let kinds_of = |group: &[&(Ways, Vec<bool>)]| -> Option<Vec<Beacon>> {
        let per_run: BTreeSet<Vec<Beacon>> = base_runs
            .iter()
            .map(|g| {
                let mut k: Vec<Beacon> = group.iter().filter_map(|((t, _), _)| g.beacons.get(t).cloned()).collect();
                k.sort();
                k
            })
            .collect();
        (per_run.len() == 1).then(|| per_run.into_iter().next()).flatten()
    };
    if !loose.is_empty() {
        let all: Vec<&(Ways, Vec<bool>)> = loose.iter().collect();
        let mut groups: Vec<Vec<&(Ways, Vec<bool>)>> = vec![all.clone()];
        if !area_centre.is_empty() {
            let mut by_area: BTreeMap<usize, Vec<&(Ways, Vec<bool>)>> = BTreeMap::new();
            for l in &loose {
                let t = l.0.0;
                let nearest = (0..area_centre.len())
                    .min_by_key(|&a| (area_centre[a].0 - t.0).pow(2) + (area_centre[a].1 - t.1).pow(2))
                    .unwrap_or(0);
                by_area.entry(nearest).or_default().push(l);
            }
            let by_area = settle_groups(by_area.into_values().collect(), base_count);
            if by_area.len() > 1 && (by_area.iter().all(|g| constant(g).is_some()) || constant(&all).is_none()) {
                groups = by_area;
            }
        }
        let mut unexplained: Vec<&(Ways, Vec<bool>)> = Vec::new();
        for g in groups {
            match constant(&g) {
                Some(k) => picked.push((k, g.iter().map(|(w, _)| w.clone()).collect(), kinds_of(&g))),
                None => unexplained.extend(g),
            }
        }
        // A few are listed; many (beacons dropped on any tile, as BoreHole does) are one note.
        if unexplained.len() > MANY_BEACONS {
            note(
                &mut notes,
                format!("{} beacon tiles appear in some games only, with no layout and no fixed number: the beacons seem dropped at random tiles (not converted)", unexplained.len()),
            );
        } else {
            for ((t, kinds), _) in unexplained {
                let kind = kinds.iter().next().map_or("beacon", |b| b.type_name.as_str());
                note(&mut notes, format!("the {kind} at {} appears in some games only, with no one layout", at(*t)));
            }
        }
    }
    let things = |o: Owner| owned.iter().find(|(x, _)| *x == o).map(|(_, t)| t.clone()).unwrap_or_default();

    // Resource levels, day and night, morale, lava, the win: at the most players, first seed.
    let reference = |lobby: Lobby| run(dll, max, seed(0), lobby);
    let base = &reference(BASE_LOBBY)?;
    let mut levels = Vec::new();
    for level in 0..3 {
        let r = if level == 0 { None } else { Some(reference(Lobby { level, ..BASE_LOBBY })?) };
        let r = r.as_ref().unwrap_or(base);
        let l = level_of(r, 0);
        if (1..max as i32).any(|p| level_of(r, p) != l) {
            note(&mut notes, format!("players don't all start alike at resource level {} (player 1's is used)", LEVEL_NAMES[level as usize]));
        }
        levels.push(l);
    }
    let off = reference(Lobby { day_night: false, steady: false, ..BASE_LOBBY })?;
    let everywhere = (last_arg(&base.calls, "TethysGame::SetDaylightEverywhere"), last_arg(&off.calls, "TethysGame::SetDaylightEverywhere"));
    let moves = last_arg(&base.calls, "TethysGame::SetDaylightMoves");
    let light = last_arg(&base.calls, "GameMap::SetInitialLightLevel");
    let (steady_calls, free_calls) = (morale_calls(&base.calls), morale_calls(&off.calls));
    // Where lava may flow (GameMap::SetLavaPossible); the functions that change tiles for lava (GameMap::SetTile)
    // are still listed as not converted.
    let lava = lava_rects(&base.calls, pad, size);
    if lava != lava_rects(&off.calls, pad, size) {
        note(&mut notes, format!("where lava may flow changes with the lobby's choices (the first game's {} areas are used)", lava.len()));
    }
    // The win: Last One Standing, Space Race, Resource Race, Midas, Land Rush (victory.rs). Land Rush's Base<N>
    // functions are measured by calling them after a setup with morale free.
    let land_rush = if base.setup.triggers.iter().any(|t| t.callback.as_deref() == Some("Base1")) {
        let start = NetStart {
            seed: seed(0),
            slots: (0..max as i32).map(|i| NetSlot { eden: true, color: i, resources: 0 }).collect(),
            day_night: true,
            disasters: true,
            morale_steady: false,
            initial_vehicles: 0,
            lift_limits: false,
            classic_rules: true,
        };
        let opts = start.options(Options { call_log: true, ..Options::from_env() });
        let exports: Vec<String> = std::iter::once("Base1".to_string()).chain((1..=max).map(|n| format!("Base{n}"))).collect();
        let calls = crate::calls::record_with(dll, &opts, 0, &exports)?;
        Some(crate::victory::LandRush { calls, players: max })
    } else {
        None
    };
    // The notes so far are the layout model's (layouts, tubes, beacons, initial vehicles): a replay has none of them.
    let model_notes = notes.len();
    let mut win_notes = Vec::new();
    let win = crate::victory::convert(&base.setup.triggers, land_rush.as_ref(), false, &mut win_notes);
    for n in win_notes {
        note(&mut notes, n);
    }
    // Disasters and timed events (timed.rs): the functions the other triggers call. Triggers the setup makes only
    // when the lobby allows disasters are found by a setup with them off.
    let calm = reference(Lobby { disasters: false, ..BASE_LOBBY })?;
    let mut left: Vec<(String, String, Option<String>)> = calm.setup.triggers.iter().map(|t| (t.kind.clone(), t.what.clone(), t.callback.clone())).collect();
    // A trigger of the base run with no match (kind, what, function) left in the calm run is gated.
    let gated: BTreeSet<u32> = base
        .setup
        .triggers
        .iter()
        .filter(|t| match left.iter().position(|k| *k == (t.kind.clone(), t.what.clone(), t.callback.clone())) {
            Some(i) => {
                left.remove(i);
                false
            }
            None => true,
        })
        .map(|t| t.id)
        .collect();
    // The computer player (ai.rs), when the mission asks for one.
    let mut ai = match desc.ai_players.unwrap_or(0) {
        0 => None,
        1 => match crate::ai::setup(dll, max, pad) {
            Ok(ai) => ai,
            Err(why) => {
                note(&mut notes, format!("the computer player isn't converted: {why}"));
                None
            }
        },
        n => {
            note(&mut notes, format!("{n} computer players (not converted: one is)"));
            None
        }
    };
    progress("disasters");
    let mut timed_notes = Vec::new();
    let timed = crate::timed::convert(dll, max, pad, &base.setup.triggers, &gated, &win.handled, &win.callbacks, ai.as_mut(), &mut timed_notes);
    for n in timed_notes {
        note(&mut notes, n);
    }
    for t in &base.setup.triggers {
        if win.handled.contains(&t.id) || timed.handled.contains(&t.id) {
            continue;
        }
        let calls = t.callback.as_deref().map_or(String::new(), |c| format!(", calls {c}"));
        note(&mut notes, format!("trigger #{} {}: {}{calls} (not converted)", t.id, t.kind, t.what));
    }
    if win.lua.is_empty() {
        note(&mut notes, "no win: none of this mission's victory conditions is converted".into());
    }
    let strays: BTreeSet<String> = base
        .setup
        .objects
        .iter()
        .filter(|o| (o.owner < 0 || o.owner >= max as i32) && !BEACON_TYPES.contains(&o.type_name.as_str()))
        .filter(|o| ai.as_ref().is_none_or(|ai| o.owner != ai.player))
        .map(|o| format!("{} at {}", o.type_name, at(o.tile)))
        .collect();
    if !strays.is_empty() {
        note(&mut notes, format!("objects owned by no player (not converted): {}", strays.into_iter().collect::<Vec<_>>().join(", ")));
    }
    if variants.values().flat_map(|v| &v.units).any(|(u, _)| u.cargo.0 > 7) {
        note(&mut notes, "a Cargo Truck starts with a load OP2Lua doesn't name (spaceport cargo, garbage or a unit; not converted)".into());
    }
    for g in &base_runs {
        for p in 0..g.players as i32 {
            let made = g.units.get(&p).map_or(0, Vec::len);
            let there = g.on_map.get(&p).copied().unwrap_or(0);
            if made != there {
                note(&mut notes, format!("player {} has units made other ways than CreateUnit ({there} on the map, {made} made)", p + 1));
            }
        }
    }

    // ---- writing ----
    let stem = dll.file_stem().map_or("mission".into(), |s| s.to_string_lossy().into_owned());
    let base_name = if simple { format!("{stem}Simple") } else { format!("{stem}Lua") };
    let name = format!("{} (Lua{})", desc.level_desc.trim(), if simple { ", simple" } else { "" });
    let file = dll.file_name().map_or(stem.clone(), |f| f.to_string_lossy().into_owned());
    let beacon_kind = |b: &Beacon| -> String {
        let ore = match b.ore { 0 => "Common", 1 => "Rare", _ => "Random" };
        let bar = match b.bar { 0 => "Bar3", 1 => "Bar2", 2 => "Bar1", _ => "Random" };
        let variant = crate::luaexport::variant_name(b.bar, b.variant);
        let kind = match b.type_name.as_str() { "MAGMA_VENT" => "MagmaVent", "FUMAROLE" => "Fumarole", _ => "MiningBeacon" };
        format!("type = \"{kind}\", ore = \"{ore}\", yield = \"{bar}\", variant = \"{variant}\"")
    };
    let beacon_line = |t: (i32, i32), b: &Beacon| -> String { format!("{{ {}, at = {} }}", beacon_kind(b), at(t)) };
    let wall_type = |w: &str| match w { "WALL" => "Wall", "WALL_LAVA" => "LavaWall", "WALL_MICROBE" => "MicrobeWall", _ => "Tube" };
    // Beacons: fixed ones as a beacon, rolled ones as the list of ways the runs saw them.
    let beacon_entry = |t: (i32, i32), kinds: &BTreeSet<Beacon>| -> String {
        let ways: Vec<String> = kinds.iter().map(|b| beacon_line(t, b)).collect();
        format!("{{ {} }}", ways.join(", "))
    };
    let write_things = |m: &mut String, ind: &str, (walls, beacons): &Things| {
        let _ = writeln!(m, "{ind}walls = {{");
        for chunk in walls.chunks(4) {
            let row: Vec<String> = chunk.iter().map(|(w, k)| format!("{{ \"{}\", {} }}", wall_type(k), at(*w))).collect();
            let _ = writeln!(m, "{ind}  {},", row.join(", "));
        }
        let _ = writeln!(m, "{ind}}},");
        let _ = writeln!(m, "{ind}beacons = {{");
        for (t, kinds) in beacons {
            let _ = writeln!(m, "{ind}  {},", beacon_entry(*t, kinds));
        }
        let _ = writeln!(m, "{ind}}},");
    };

    let (fixed_walls, fixed_beacons) = things(Owner::All);
    let mut p = String::new();
    let _ = writeln!(p, "-- {base_name}.placement.lua  -  {name}");
    let _ = writeln!(p, "-- Converted by op2dll from {file}: what its setup makes in every game, whatever the lobby chose.");
    let _ = writeln!(p, "-- Each base's layout, tubes and beacons come with the player who gets it ({base_name}.lua). No");
    let _ = writeln!(p, "-- `players` table: in a multiplayer game the lobby sets each player's colony and colour.");
    let _ = writeln!(p, "-- Coordinates are the in-game status-bar ones (map file tile + 1).");
    let _ = writeln!(p, "return {{");
    let _ = writeln!(p, "  name = {}, map = {}, tech = {},", quoted(&name), quoted(&desc.map_name), quoted(&desc.techtree_name));
    let _ = writeln!(p, "  type = \"{}\", max_tech = {}, players_count = {},", crate::opm::mission_type(desc.mission_type), desc.max_tech_level, max);
    let _ = writeln!(p, "\n  walls = {{");
    for (t, w) in &fixed_walls {
        let _ = writeln!(p, "    {{ type = \"{}\", at = {} }},", wall_type(w), at(*t));
    }
    let _ = writeln!(p, "  }},\n\n  beacons = {{");
    for (t, kinds) in fixed_beacons.iter().filter(|(_, k)| k.len() == 1) {
        let _ = writeln!(p, "    {},", beacon_line(*t, kinds.iter().next().expect("one")));
    }
    let _ = writeln!(p, "  }},\n}}");

    let mut m = String::new();
    if simple {
        let _ = writeln!(m, "-- {base_name}.lua  -  {name}");
        let _ = writeln!(m, "--");
        let _ = writeln!(m, "-- A simple version of {file} (\"{}\"), converted by op2dll.", desc.level_desc.trim());
        let _ = writeln!(m, "-- Each area has one base, the same every game, where the original rolls one of several layouts,");
        let _ = writeln!(m, "-- and the beacons are fixed. Who gets which area, the win, disasters, day and night, morale and");
        let _ = writeln!(m, "-- any computer player are the original's. Coordinates are the in-game status-bar ones (map file");
        let _ = writeln!(m, "-- tile + 1). The mission log");
        let _ = writeln!(m, "-- (OPU\\logs\\OP2Lua-{base_name}.log) records the lobby's settings and who got which base.\n");
        let _ = writeln!(m, "-- Each area's base: structures (type, tile, weapon), vehicles (type, tile, facing, weapon; a Cargo");
        let _ = writeln!(m, "-- Truck's load and amount), walls and tubes, beacons, where the camera starts and where the lobby's");
        let _ = writeln!(m, "-- initial vehicles are dropped. Every resource level gets the same units (the original's at High).");
        let _ = writeln!(m, "local BASES = {{");
        let rows = |m: &mut String, key: &str, items: &[String], per: usize| {
            if items.is_empty() {
                let _ = writeln!(m, "    {key} = {{}},");
                return;
            }
            let _ = writeln!(m, "    {key} = {{");
            for chunk in items.chunks(per) {
                let _ = writeln!(m, "      {},", chunk.join(", "));
            }
            let _ = writeln!(m, "    }},");
        };
        // An area with more than one base (see the picks) names the others after it: N, N_2...
        let bases = areas.iter().enumerate().flat_map(|(i, a)| chosen[i].iter().enumerate().map(move |(j, p)| (i, a, j, p)));
        for (i, a, j, (t, on_rock, way, shift)) in bases {
            let key = if j == 0 { area_names[i].clone() } else { format!("{}_{}", area_names[i], j + 1) };
            let (t, v) = (*t, &variants[t]);
            let skipped = if on_rock.is_empty() {
                String::new()
            } else {
                let names: Vec<String> = on_rock.iter().map(|t| variant_name(*t)).collect();
                format!("; {} put a structure on blocked ground", names.join(", "))
            };
            let _ = writeln!(m, "  {key} = {{  -- the original's {}, one of {} here{skipped}", variant_name(t), a.len());
            // A column start in the way picked for it, unit by unit (its arrangements list the units in one order).
            let column = way.and_then(|f| columns.get(&t)?.get(&f)).filter(|c| c.len() == v.units.len());
            // The shift moves a column whose every way is blocked (see the Pick); nothing else moves.
            let by = if column.is_some() { *shift } else { (0, 0) };
            let moved = |p: (i32, i32)| (p.0 + by.0, p.1 + by.1);
            if let (Some(_), Some(f)) = (column, way) {
                let _ = writeln!(m, "    -- its column faces {}: the original rolls one of {} ways", DIRECTIONS[*f as usize], columns[&t].len());
                if by != (0, 0) {
                    let _ = writeln!(
                        m,
                        "    -- moved {} east and {} south of the original's start tile: every way puts a unit on blocked ground there",
                        by.0, by.1
                    );
                }
            }
            if let Some(view) = v.view {
                let _ = writeln!(m, "    view = {},", at(moved(view)));
            }
            let turned: Vec<(Unit, [bool; 3])> = match column {
                Some(c) => v.units.iter().zip(c).map(|((u, l), cu)| (Unit { tile: moved(cu.tile), rotation: cu.rotation, ..u.clone() }, *l)).collect(),
                None => v.units.clone(),
            };
            let mut units: Vec<&Unit> = turned.iter().filter(|(_, l)| l[0]).map(|(u, _)| u).collect();
            units.sort();
            let weapon = |u: &Unit| if u.weapon > 0 { format!(", \"{}\"", lua_name(u.weapon)) } else { String::new() };
            let load = |u: &Unit| match cargo_name(u.cargo.0) {
                Some(c) => format!(", cargo = {{ \"{c}\", {} }}", u.cargo.1),
                None => String::new(),
            };
            let structures: Vec<String> =
                units.iter().filter(|u| !u.vehicle).map(|u| format!("{{ \"{}\", {}{} }}", lua_name(u.type_id), at(u.tile), weapon(u))).collect();
            let vehicles: Vec<String> = units
                .iter()
                .filter(|u| u.vehicle)
                .map(|u| format!("{{ \"{}\", {}, \"{}\"{}{} }}", lua_name(u.type_id), at(u.tile), DIRECTIONS[u.rotation as usize], weapon(u), load(u)))
                .collect();
            rows(&mut m, "structures", &structures, 3);
            rows(&mut m, "vehicles", &vehicles, 3);
            let (mut walls, mut beacons) = things(Owner::Variant(t));
            // The area's own tubes, walls and beacons go with its first base only.
            if j == 0 {
                let (area_walls, area_beacons) = things(Owner::Area(i));
                walls.extend(area_walls);
                beacons.extend(area_beacons);
            }
            let walls: Vec<String> = walls.iter().map(|(w, k)| format!("{{ \"{}\", {} }}", wall_type(k), at(*w))).collect();
            rows(&mut m, "walls", &walls, 4);
            // A beacon the original makes one of several ways is made the first of them.
            let beacons: Vec<String> = beacons.iter().filter_map(|(t, k)| k.iter().next().map(|b| beacon_line(*t, b))).collect();
            rows(&mut m, "beacons", &beacons, 1);
            let spots = match way.and(column).and_then(|_| column_spots.get(&(t, (*way)?))) {
                Some(s) if !s.is_empty() => s,
                _ => &v.spots,
            };
            if !spots.is_empty() {
                let spots: Vec<String> = spots.iter().map(|s| at(moved(*s))).collect();
                let _ = writeln!(m, "    vehicle_spots = {{ {} }},", spots.join(", "));
            }
            let _ = writeln!(m, "  }},");
        }
        let _ = writeln!(m, "}}\n");
    } else {
        let _ = writeln!(m, "-- {base_name}.lua  -  {name}");
        let _ = writeln!(m, "--");
        let _ = writeln!(m, "-- Converted by op2dll from {file} (\"{}\"): written from what its setup does under every lobby", desc.level_desc.trim());
        let _ = writeln!(m, "-- choice, not from its code. Check it in a game; anything not converted is listed at the end.");
        let _ = writeln!(m, "-- Coordinates are the in-game status-bar ones (map file tile + 1). The mission log");
        let _ = writeln!(m, "-- (OPU\\logs\\OP2Lua-{base_name}.log) records the lobby's settings and who got which base.\n");
        let _ = writeln!(m, "-- Each base layout: its units (levels = the resource levels that get one, when not all), tubes and");
        let _ = writeln!(m, "-- walls, beacons (each made one of the ways listed), camera, and where initial vehicles are dropped.");
        let _ = writeln!(m, "-- A layout with `starts` is one kit placed on a start tile rolled from that list: its other");
        let _ = writeln!(m, "-- coordinates are offsets from the start tile.");
        if !columns.is_empty() {
            let _ = writeln!(m, "-- A layout with `columns` is laid out one of those ways, rolled each game: the way its units face,");
            let _ = writeln!(m, "-- and each unit's tile, in the order of `units`.");
        }
        let _ = writeln!(m, "local LAYOUTS = {{");
        // A spread layout is written once, at its first tile, with every tile it was seen at as a start.
        let rel = |(x, y): (i32, i32)| format!("{{ {x}, {y} }}");
        for (t, v) in &variants {
            let group = spread_of.get(t).map(|&g| &spread[g]);
            if group.is_some_and(|g| g[0] != *t) {
                continue;
            }
            let origin = group.map(|_| *t);
            let place = |p: (i32, i32)| match origin {
                Some(o) => rel((p.0 - o.0, p.1 - o.1)),
                None => at(p),
            };
            let _ = writeln!(m, "  {} = {{", variant_name(*t));
            if let Some(g) = group {
                let (x1, x2) = (g.iter().map(|s| s.0).min().unwrap_or(0), g.iter().map(|s| s.0).max().unwrap_or(0));
                let (y1, y2) = (g.iter().map(|s| s.1).min().unwrap_or(0), g.iter().map(|s| s.1).max().unwrap_or(0));
                let area = ((x2 - x1 + 1) * (y2 - y1 + 1)) as usize;
                if g.len() >= START_AREA_MIN && g.len() * START_FILL >= area {
                    let _ = writeln!(m, "    start_area = {{ {}, {}, {}, {} }},  -- {} starts seen in it", x1 + 1, y1 + 1, x2 + 1, y2 + 1, g.len());
                } else {
                    let rows: Vec<String> = g.chunks(8).map(|c| c.iter().map(|s| at(*s)).collect::<Vec<_>>().join(", ")).collect();
                    let _ = writeln!(m, "    starts = {{\n      {},\n    }},", rows.join(",\n      "));
                }
            }
            if let Some(view) = v.view {
                let _ = writeln!(m, "    view = {},", place(view));
            }
            let _ = writeln!(m, "    units = {{");
            let mut order: Vec<usize> = (0..v.units.len()).collect();
            order.sort_by(|&a, &b| v.units[a].cmp(&v.units[b]));
            let units: Vec<(Unit, [bool; 3])> = order.iter().map(|&i| v.units[i].clone()).collect();
            for (u, lv) in &units {
                let stacked = units.iter().filter(|(o, _)| o.tile == u.tile).count() > 1;
                let weapon = if u.weapon > 0 { format!(", weapon = \"{}\"", lua_name(u.weapon)) } else { String::new() };
                // Structures and units on a tile of their own go exactly there; vehicles the original stacks on one
                // tile are spread by the game's own placement finder.
                let exact = if !u.vehicle || !stacked { ", exact = true" } else { "" };
                let dir = if u.vehicle { format!(", direction = \"{}\"", DIRECTIONS[u.rotation as usize]) } else { String::new() };
                let only = if lv.iter().all(|&x| x) {
                    String::new()
                } else {
                    let names: Vec<String> = (0..3).filter(|&i| lv[i]).map(|i| quoted(LEVEL_NAMES[i])).collect();
                    format!(", levels = {{ {} }}", names.join(", "))
                };
                let load = cargo_name(u.cargo.0).map_or(String::new(), |c| format!(", cargo = \"{c}\", amount = {}", u.cargo.1));
                let _ = writeln!(m, "      {{ type = \"{}\", at = {}{dir}{weapon}{load}{exact}{only} }},", lua_name(u.type_id), place(u.tile));
            }
            let _ = writeln!(m, "    }},");
            match columns.get(t) {
                Some(forms) if forms.values().all(|c| c.len() == v.units.len()) => {
                    let _ = writeln!(m, "    columns = {{");
                    for (f, c) in forms {
                        let tiles: Vec<String> = order.iter().map(|&i| place(c[i].tile)).collect();
                        let rows: Vec<String> = tiles.chunks(8).map(|r| r.join(", ")).collect();
                        let spots = match column_spots.get(&(*t, *f)).filter(|s| !s.is_empty()) {
                            Some(s) => format!("\n        vehicle_spots = {{ {} }},", s.iter().map(|s| place(*s)).collect::<Vec<_>>().join(", ")),
                            None => String::new(),
                        };
                        let _ = writeln!(
                            m,
                            "      {{ facing = \"{}\", tiles = {{\n        {},\n        }},{spots}\n      }},",
                            DIRECTIONS[*f as usize],
                            rows.join(",\n        ")
                        );
                    }
                    let _ = writeln!(m, "    }},");
                }
                Some(_) => note(&mut notes, format!("the column at {} has units only some resource levels get (its ways not converted)", at(*t))),
                None => {}
            }
            write_things(&mut m, "    ", &things(Owner::Variant(*t)));
            let own_spots = columns.get(t).is_some_and(|forms| forms.keys().all(|f| column_spots.get(&(*t, *f)).is_some_and(|s| !s.is_empty())));
            if !v.spots.is_empty() && !own_spots {
                let spots: Vec<String> = v.spots.iter().map(|s| place(*s)).collect();
                let _ = writeln!(m, "    vehicle_spots = {{ {} }},", spots.join(", "));
            }
            let _ = writeln!(m, "  }},");
        }
        let _ = writeln!(m, "}}\n");

        let _ = writeln!(m, "-- Areas: each is one of its layouts, rolled when a player gets it, plus what any of them brings.");
        let _ = writeln!(m, "local AREAS = {{");
        for (i, a) in areas.iter().enumerate() {
            // A spread layout once if it's the area's only one; else once per start, so each start tile is as likely
            // as each fixed layout.
            let only_spread = a.iter().all(|t| spread_of.get(t) == spread_of.get(&a[0])) && spread_of.contains_key(&a[0]);
            let names: Vec<String> = a
                .iter()
                .filter(|t| !only_spread || **t == a[0])
                .map(|t| quoted(&variant_name(spread_of.get(t).map_or(*t, |&g| spread[g][0]))))
                .collect();
            let _ = writeln!(m, "  {} = {{", area_names[i]);
            let _ = writeln!(m, "    layouts = {{ {} }},", names.join(", "));
            write_things(&mut m, "    ", &things(Owner::Area(i)));
            let _ = writeln!(m, "  }},");
        }
        let _ = writeln!(m, "}}\n");
    }

    let _ = writeln!(m, "-- Who gets which area, by player count: one of these is picked at random (player n gets the nth).");
    let _ = writeln!(m, "local ASSIGN = {{");
    for (n, list) in &handouts {
        let mut by_area: BTreeSet<Vec<String>> = BTreeSet::new();
        for h in list {
            // In the simple version an area's second player in a hand-out gets its second base (N_2), and so on.
            let mut seen: BTreeMap<usize, usize> = BTreeMap::new();
            by_area.insert(
                h.iter()
                    .map(|t| match area_of.get(t) {
                        Some(&a) => {
                            let k = seen.entry(a).or_default();
                            *k += 1;
                            if simple && *k > 1 { format!("{}_{k}", area_names[a]) } else { area_names[a].clone() }
                        }
                        None => "?".into(),
                    })
                    .collect(),
            );
        }
        let rows: Vec<String> = by_area.iter().map(|a| format!("{{ {} }}", a.iter().map(|s| quoted(s)).collect::<Vec<_>>().join(", "))).collect();
        let _ = writeln!(m, "  [{n}] = {{ {} }},", rows.join(", "));
    }
    let _ = writeln!(m, "}}\n");

    let _ = writeln!(m, "-- By the lobby's resource level (player.difficulty: \"Easy\" is High, \"Hard\" Low): colonists, stores,");
    let _ = writeln!(m, "-- research, and the Structure Factory's kits by bay.");
    let _ = writeln!(m, "local LEVELS = {{");
    for (i, l) in levels.iter().enumerate() {
        let nums: Vec<String> = l.numbers.iter().map(|(k, v)| format!("{k} = {v}")).collect();
        let research: Vec<String> = l.research.iter().map(i32::to_string).collect();
        let kits: Vec<String> = l
            .kits
            .iter()
            .map(|(bay, item, weapon)| {
                if *weapon > 0 {
                    format!("[{bay}] = {{ \"{}\", \"{}\" }}", lua_name(*item), lua_name(*weapon))
                } else {
                    format!("[{bay}] = {{ \"{}\" }}", lua_name(*item))
                }
            })
            .collect();
        let _ = writeln!(m, "  {} = {{", LEVEL_NAMES[i]);
        let _ = writeln!(m, "    {},", nums.join(", "));
        let _ = writeln!(m, "    research = {{ {} }},", research.join(", "));
        if let Some(tl) = l.tech_level {
            let _ = writeln!(m, "    tech_level = {tl},");
        }
        let _ = writeln!(m, "    kits = {{ {} }},", kits.join(", "));
        let _ = writeln!(m, "  }},");
    }
    let _ = writeln!(m, "}}\n");

    let weapons: Vec<String> = plymouth_weapons.iter().map(|(e, p)| format!("{} = \"{}\"", lua_name(*e), lua_name(*p))).collect();
    let _ = writeln!(m, "-- The weapons a Plymouth player's units get instead of Eden's.");
    let _ = writeln!(m, "local PLYMOUTH_WEAPONS = {{ {} }}\n", weapons.join(", "));
    let vehicle = |v: Option<(i32, i32)>| {
        v.map_or("nil".to_string(), |(t, w)| {
            let weapon = if w > 0 { format!(", weapon = \"{}\"", lua_name(w)) } else { String::new() };
            format!("{{ type = \"{}\"{weapon} }}", lua_name(t))
        })
    };
    let _ = writeln!(m, "-- The lobby's Initial Vehicles, by colony: each is dropped on one of its {}'s vehicle_spots.", if simple { "base" } else { "layout" });
    let _ = writeln!(m, "local INITIAL = {{ eden = {}, plymouth = {} }}\n", vehicle(initial[0]), vehicle(initial[1].or(initial[0])));
    if !lava.is_empty() {
        let tiles: i32 = lava.iter().map(|r| (r[2] - r[0] + 1) * (r[3] - r[1] + 1)).sum();
        let _ = writeln!(m, "-- Where lava may flow from an eruption: {tiles} tiles, as areas {{ x1, y1, x2, y2 }}.");
        let _ = writeln!(m, "local LAVA_POSSIBLE = {{");
        for chunk in lava.chunks(6) {
            let row: Vec<String> = chunk.iter().map(|r| format!("{{ {}, {}, {}, {} }}", r[0], r[1], r[2], r[3])).collect();
            let _ = writeln!(m, "  {},", row.join(", "));
        }
        let _ = writeln!(m, "}}\n");
    }

    if simple {
        // Beacons the original rolls (made one of several ways, or on some spots of a list), fixed: the first way;
        // a list's `count` spots spread evenly through it, with its kinds when every game makes the same ones.
        let mut fixed: Vec<String> = fixed_beacons.iter().filter(|(_, k)| k.len() > 1).filter_map(|(t, k)| k.iter().next().map(|b| beacon_line(*t, b))).collect();
        for (k, spots, kinds) in &picked {
            for i in 0..*k {
                let (t, ways) = &spots[i * spots.len() / k];
                match kinds.as_ref().and_then(|kinds| kinds.get(i)) {
                    Some(b) => fixed.push(beacon_line(*t, b)),
                    None => fixed.extend(ways.iter().next().map(|b| beacon_line(*t, b))),
                }
            }
        }
        let _ = writeln!(m, "-- The beacons not in a base: where the original rolls them, one fixed pick.");
        let _ = writeln!(m, "local BEACONS = {{");
        for b in &fixed {
            let _ = writeln!(m, "  {b},");
        }
        let _ = writeln!(m, "}}\n");
    } else {
        let rolled: Vec<String> = fixed_beacons.iter().filter(|(_, k)| k.len() > 1).map(|(t, k)| format!("  {},", beacon_entry(*t, k))).collect();
        let _ = writeln!(m, "-- Beacons on every map, rolled each game: each is made one of the ways listed.");
        let _ = writeln!(m, "local ROLLED_BEACONS = {{\n{}\n}}\n", rolled.join("\n"));
        let _ = writeln!(m, "-- Lists of beacon spots: every game has `count` of each list's spots, picked at random. With");
        let _ = writeln!(m, "-- `kinds`, those beacons go on the picked spots; else each spot is made one of the ways listed.");
        let _ = writeln!(m, "local BEACON_SPOTS = {{");
        for (k, spots, kinds) in &picked {
            match kinds {
                Some(kinds) => {
                    let _ = writeln!(m, "  {{ count = {k}, kinds = {{");
                    for b in kinds {
                        let _ = writeln!(m, "      {{ {} }},", beacon_kind(b));
                    }
                    let tiles: Vec<String> = spots.iter().map(|(t, _)| at(*t)).collect();
                    let rows: Vec<String> = tiles.chunks(8).map(|c| c.join(", ")).collect();
                    let _ = writeln!(m, "    }},\n    spots = {{\n      {},\n    }} }},", rows.join(",\n      "));
                }
                None => {
                    let _ = writeln!(m, "  {{ count = {k}, spots = {{");
                    for (t, kinds) in spots {
                        let _ = writeln!(m, "    {},", beacon_entry(*t, kinds));
                    }
                    let _ = writeln!(m, "  }} }},");
                }
            }
        }
        let _ = writeln!(m, "}}\n");
    }
    // From here on, `m`'s lines are the body of `on_init`, which the runtime opens.
    m.push_str(if simple { SIMPLE_RUNTIME } else { FAITHFUL_RUNTIME });
    // With a computer player, the bases and the lobby's resources are the humans'; the computer player comes after.
    let (count, ai_call) = match &ai {
        Some(_) => ("num_humans", "  setup_ai(players[n + 1])\n"),
        None => ("num_players", ""),
    };
    m = m.replacen("game.{count}()", &format!("game.{count}()"), 1).replacen("{ai_call}", ai_call, 1);
    // SetDaylightEverywhere with Day and Night on, then off: off with it and on without it follows the lobby.
    match everywhere {
        (Some(0), Some(1)) => m.push_str("  game.set_daylight_everywhere(not game.uses_day_night())\n"),
        (Some(v), Some(w)) if v == w => {
            let _ = writeln!(m, "  game.set_daylight_everywhere({})", v != 0);
        }
        (None, None) => {}
        other => note(&mut notes, format!("SetDaylightEverywhere follows the lobby in a way not converted ({other:?})")),
    }
    if let Some(v) = moves {
        let _ = writeln!(m, "  game.set_daylight_moves({})", v != 0);
    }
    if let Some(v) = light {
        let _ = writeln!(m, "  game.set_initial_light_level({v})");
    }
    if !lava.is_empty() {
        m.push_str("  for _, area in ipairs(LAVA_POSSIBLE) do game.set_lava_possible(area) end\n");
    }
    if free_calls.starts_with(&steady_calls) {
        // Morale Steady's calls always, and some more when the lobby frees morale.
        for c in &steady_calls {
            let _ = writeln!(m, "  {c}");
        }
        let extra = &free_calls[steady_calls.len()..];
        if !extra.is_empty() {
            let _ = writeln!(m, "  if game.uses_morale() then");
            for c in extra {
                let _ = writeln!(m, "    {c}");
            }
            let _ = writeln!(m, "  end");
        }
    } else {
        // One set of calls or the other, by the lobby's Morale Steady.
        let _ = writeln!(m, "  if game.uses_morale() then");
        for c in &free_calls {
            let _ = writeln!(m, "    {c}");
        }
        let _ = writeln!(m, "  else");
        for c in &steady_calls {
            let _ = writeln!(m, "    {c}");
        }
        let _ = writeln!(m, "  end");
    }
    // The converted trigger functions go before on_init, which makes their triggers.
    m = m.replacen("
function on_init()
", &format!("
{}{}function on_init()
", ai.as_ref().map_or(String::new(), |a| a.header()), timed.functions), 1);
    m.push_str(&win.lua);
    m.push_str(&timed.init);
    m.push_str("  return true\nend\n");
    if !notes.is_empty() {
        m.push_str("\n-- Not converted (op2dll):\n");
        for n in &notes {
            let _ = writeln!(m, "--   {n}");
        }
    }
    let ai_players = i32::from(ai.is_some());
    // The faithful mission's setup as a replay of the original's (replay.rs): the same draws and the same calls in the
    // same order, so its games are the original's. A computer player's setup is ai.rs's `setup_ai`, called where the
    // original makes it. The replay's notes are the win's and the timed functions' only (`model_notes` on).
    if !simple {
        progress("replay");
        match crate::replay::convert(dll, max, pad, ai.is_some(), progress) {
            Ok((head, body)) => {
                // The replay makes every object, those owned by no player (wrecks) too.
                let notes: Vec<String> = notes[model_notes..].iter().filter(|n| !n.starts_with("objects owned by no player")).cloned().collect();
                let mut r = String::new();
                let _ = writeln!(r, "-- {base_name}.lua  -  {name}");
                let _ = writeln!(r, "--");
                let _ = writeln!(r, "-- Converted by op2dll from {file} (\"{}\"): its setup replayed. `on_init` makes the original's", desc.level_desc.trim());
                let _ = writeln!(r, "-- draws of the game's random numbers in its order and, from them and the lobby, the same units,");
                let _ = writeln!(r, "-- beacons, walls and settings in the same order, as the original's setup did in every game");
                let _ = writeln!(r, "-- sampled (every player count, the lobby's colonies, resource levels, vehicles, day and night,");
                let _ = writeln!(r, "-- morale and disasters). Anything not converted is listed at the end. Coordinates are the in-game");
                let _ = writeln!(r, "-- status-bar ones (map file tile + 1).\n");
                if !lava.is_empty() {
                    let _ = writeln!(r, "-- Where lava may flow from an eruption, as areas {{ x1, y1, x2, y2 }}.");
                    let _ = writeln!(r, "local LAVA_POSSIBLE = {{");
                    for chunk in lava.chunks(6) {
                        let row: Vec<String> = chunk.iter().map(|a| format!("{{ {}, {}, {}, {} }}", a[0], a[1], a[2], a[3])).collect();
                        let _ = writeln!(r, "  {},", row.join(", "));
                    }
                    let _ = writeln!(r, "}}\n");
                }
                r.push_str(&head);
                r.push('\n');
                // The computer player's helpers and `setup_ai`; `n` then counts the humans.
                r.push_str(&ai.as_ref().map_or(String::new(), |a| a.header()));
                r.push_str(&timed.functions);
                let count = if ai.is_some() { "num_humans" } else { "num_players" };
                let _ = write!(r, "function on_init()\n  local n = game.{count}()\n");
                r.push_str(&body);
                r.push_str(&win.lua);
                r.push_str(&timed.init);
                r.push_str("  return true\nend\n");
                if !notes.is_empty() {
                    r.push_str("\n-- Not converted (op2dll):\n");
                    for n in &notes {
                        let _ = writeln!(r, "--   {n}");
                    }
                }
                let mut pl = String::new();
                let _ = writeln!(pl, "-- {base_name}.placement.lua  -  {name}");
                let _ = writeln!(pl, "-- Converted by op2dll from {file}. The setup is replayed in {base_name}.lua, which makes every unit,");
                let _ = writeln!(pl, "-- beacon and wall in the original's order, so the placement makes none.");
                let _ = writeln!(pl, "return {{");
                let _ = writeln!(pl, "  name = {}, map = {}, tech = {},", quoted(&name), quoted(&desc.map_name), quoted(&desc.techtree_name));
                let _ = writeln!(pl, "  type = \"{}\", max_tech = {}, players_count = {},", crate::opm::mission_type(desc.mission_type), desc.max_tech_level, max);
                let _ = writeln!(pl, "}}");
                return Ok(Lua { base: base_name, name, placement: pl, mission: r, notes, ai_players });
            }
            Err(why) => {
                // The notes block written above, or its header when there were none.
                if notes.is_empty() {
                    m.push_str("\n-- Not converted (op2dll):\n");
                }
                note(&mut notes, format!("{why} (the setup is the model's, not a replay)"));
                m.push_str(&format!("--   {why} (the setup is the model's, not a replay)\n"));
            }
        }
    }
    Ok(Lua { base: base_name, name, placement: p, mission: m, notes, ai_players })
}

/// The faithful mission's functions and `on_init` up to the computer player; `{count}` and `{ai_call}` are filled in.
const FAITHFUL_RUNTIME: &str = r#"local function on_off(on) return on and "on" or "off" end

local function has(list, x)
  for _, v in ipairs(list) do if v == x then return true end end
  return false
end

-- Walls, tubes and beacons a layout or area brings.
local function build_things(t)
  for _, w in ipairs(t.walls) do
    if w[1] == "Tube" then game.create_tube(w[2]) else game.create_wall(w[1], w[2]) end
  end
  for _, ways in ipairs(t.beacons) do game.create_beacon(ways[#ways > 1 and game.rand(#ways) + 1 or 1]) end
end

-- Builds `layout` for `player`; returns the start tile it rolled, for a layout with `starts`, and the way its
-- column faces, for a layout with `columns`.
local function build_layout(player, area, layout)
  local difficulty = LEVELS[player.difficulty] and player.difficulty or "Easy"
  local level = LEVELS[difficulty]
  local start = layout.starts and layout.starts[game.rand(#layout.starts) + 1]
  local a = layout.start_area
  if a then start = { a[1] + game.rand(a[3] - a[1] + 1), a[2] + game.rand(a[4] - a[2] + 1) } end
  local function place(p) return start and { start[1] + p[1], start[2] + p[2] } or p end
  local column = layout.columns and layout.columns[game.rand(#layout.columns) + 1]
  for i, u in ipairs(layout.units) do
    if not u.levels or has(u.levels, difficulty) then
      local weapon = u.weapon
      if weapon and not player.is_eden then weapon = PLYMOUTH_WEAPONS[weapon] or weapon end
      local at, direction = u.at, u.direction
      if column then at, direction = column.tiles[i], column.facing end
      local made = game.create_unit{ type = u.type, player = player, at = place(at), direction = direction,
                                     weapon = weapon, exact = u.exact }
      if u.type == "StructureFactory" and made[1] then
        for bay, kit in pairs(level.kits) do made[1]:set_factory_cargo(bay, kit[1], kit[2]) end
      end
      if u.cargo and made[1] then made[1]:set_cargo(u.cargo, u.amount) end
    end
  end
  build_things(layout)
  build_things(area)
  local initial = player.is_eden and INITIAL.eden or INITIAL.plymouth
  local spots = column and column.vehicle_spots or layout.vehicle_spots
  if initial and spots then
    for _ = 1, game.initial_units() do
      local spot = spots[game.rand(#spots) + 1]
      game.create_unit{ type = initial.type, player = player, at = place(spot), weapon = initial.weapon }
    end
  end
  if layout.view then player:center_view(place(layout.view)) end
  return start, column and column.facing
end

local function set_resources(player)
  local level = LEVELS[player.difficulty] or LEVELS.Easy
  if level.tech_level then player:set_tech_level(level.tech_level) end
  for _, tech in ipairs(level.research) do player:mark_research(tech) end
  player.workers, player.scientists, player.kids = level.workers, level.scientists, level.kids
  player.food, player.ore, player.rare_ore = level.food, level.ore, level.rare_ore
end

function on_init()
  local n = game.{count}()
  local choices = ASSIGN[n] or ASSIGN[#ASSIGN]
  local assign = choices[#choices > 1 and game.rand(#choices) + 1 or 1]
  print(string.format("Lobby: %d players; morale steady %s, day and night %s, disasters %s, %d initial vehicles",
                      n, on_off(not game.uses_morale()), on_off(game.uses_day_night()),
                      on_off(game.disasters_enabled()), game.initial_units()))
  for id = 1, n do
    local p = players[id]
    local area = AREAS[assign[id]]
    local name = area.layouts[#area.layouts > 1 and game.rand(#area.layouts) + 1 or 1]
    local start, facing = build_layout(p, area, LAYOUTS[name])
    print(string.format("Player %d: %s, %s, resources %s, area %s, layout %s%s%s", id, p.is_eden and "Eden" or "Plymouth",
                        p.is_human and "human" or "computer", p.difficulty, assign[id], name,
                        start and string.format(" at %d,%d", start[1], start[2]) or "",
                        facing and " facing " .. facing or ""))
  end
  for _, ways in ipairs(ROLLED_BEACONS) do game.create_beacon(ways[game.rand(#ways) + 1]) end
  for _, list in ipairs(BEACON_SPOTS) do
    local spots = {}
    for i, s in ipairs(list.spots) do spots[i] = s end
    for i = 1, list.count do
      local spot = table.remove(spots, game.rand(#spots) + 1)
      if list.kinds then
        local b = {}
        for k, v in pairs(list.kinds[i]) do b[k] = v end
        b.at = spot
        game.create_beacon(b)
      else
        game.create_beacon(spot[#spot > 1 and game.rand(#spot) + 1 or 1])
      end
    end
  end
  for id = 1, n do set_resources(players[id]) end
{ai_call}
"#;

/// The simple mission's functions and `on_init` up to the computer player, as `FAITHFUL_RUNTIME`.
const SIMPLE_RUNTIME: &str = r#"local function on_off(on) return on and "on" or "off" end

local function weapon_for(player, weapon)
  if weapon and not player.is_eden then return PLYMOUTH_WEAPONS[weapon] or weapon end
  return weapon
end

-- Builds `base` for `player`. Vehicles the original stacks on one tile are spread by the game.
local function build_base(player, base)
  local level = LEVELS[player.difficulty] or LEVELS.Easy
  for _, s in ipairs(base.structures) do
    local made = game.create_unit{ type = s[1], player = player, at = s[2], weapon = weapon_for(player, s[3]), exact = true }
    if s[1] == "StructureFactory" and made[1] then
      for bay, kit in pairs(level.kits) do made[1]:set_factory_cargo(bay, kit[1], kit[2]) end
    end
  end
  local on_tile = {}
  for _, v in ipairs(base.vehicles) do
    local key = v[2][1] .. "," .. v[2][2]
    on_tile[key] = (on_tile[key] or 0) + 1
  end
  for _, v in ipairs(base.vehicles) do
    local made = game.create_unit{ type = v[1], player = player, at = v[2], direction = v[3], weapon = weapon_for(player, v[4]),
                                   exact = on_tile[v[2][1] .. "," .. v[2][2]] == 1 }
    if v.cargo and made[1] then made[1]:set_cargo(v.cargo[1], v.cargo[2]) end
  end
  for _, w in ipairs(base.walls) do
    if w[1] == "Tube" then game.create_tube(w[2]) else game.create_wall(w[1], w[2]) end
  end
  for _, b in ipairs(base.beacons) do game.create_beacon(b) end
  local initial = player.is_eden and INITIAL.eden or INITIAL.plymouth
  if initial and base.vehicle_spots then
    for _ = 1, game.initial_units() do
      local spot = base.vehicle_spots[game.rand(#base.vehicle_spots) + 1]
      game.create_unit{ type = initial.type, player = player, at = spot, weapon = initial.weapon }
    end
  end
  if base.view then player:center_view(base.view) end
end

local function set_resources(player)
  local level = LEVELS[player.difficulty] or LEVELS.Easy
  if level.tech_level then player:set_tech_level(level.tech_level) end
  for _, tech in ipairs(level.research) do player:mark_research(tech) end
  player.workers, player.scientists, player.kids = level.workers, level.scientists, level.kids
  player.food, player.ore, player.rare_ore = level.food, level.ore, level.rare_ore
end

function on_init()
  local n = game.{count}()
  local choices = ASSIGN[n] or ASSIGN[#ASSIGN]
  local assign = choices[#choices > 1 and game.rand(#choices) + 1 or 1]
  print(string.format("Lobby: %d players; morale steady %s, day and night %s, disasters %s, %d initial vehicles",
                      n, on_off(not game.uses_morale()), on_off(game.uses_day_night()),
                      on_off(game.disasters_enabled()), game.initial_units()))
  for id = 1, n do
    local p = players[id]
    build_base(p, BASES[assign[id]])
    print(string.format("Player %d: %s, %s, resources %s, base %s", id, p.is_eden and "Eden" or "Plymouth",
                        p.is_human and "human" or "computer", p.difficulty, assign[id]))
  end
  for _, b in ipairs(BEACONS) do game.create_beacon(b) end
  for id = 1, n do set_resources(players[id]) end
{ai_call}
"#;

/// OP2Lua's name for a Cargo Truck's load (`TruckCargo` 1 to 10); None for an empty truck. The amount goes with it as
/// the game has it: Tutorial P's trucks carry spacecraft (amounts 91 and 92) and wreckage (801).
pub(crate) fn cargo_name(c: i32) -> Option<&'static str> {
    /// OP2Lua's load names, from `TruckCargo` 1.
    const NAMES: [&str; 10] = [
        "Food", "CommonOre", "RareOre", "CommonMetal", "RareMetal", "CommonRubble", "RareRubble", "Spacecraft", "Wreckage", "GeneBank",
    ];
    usize::try_from(c - 1).ok().and_then(|i| NAMES.get(i)).copied()
}

/// A unit as a Plymouth player gets it: its weapon swapped as Plymouth's are.
fn colony(u: &Unit, weapons: &BTreeMap<i32, i32>) -> Unit {
    Unit { weapon: weapons.get(&u.weapon).copied().unwrap_or(u.weapon), ..u.clone() }
}

/// Placeholders in OP2Lua's prebuilt stub (`LuaMission.dll`), as op2lua-newmission patches them: the menu name.
const LEVEL_DESC: &str = "OP2Lua mission (unpatched template)";
/// The stub's map file name, replaced by the mission's.
const MAP_NAME: &str = "eden01.map";
/// The stub's tech tree file name, replaced when the mission's is another.
const TECH_TREE: &str = "MULTITEK.TXT";
/// The stub's version-resource FileDescription (UTF-16), replaced by the menu name's ASCII characters, cut to its length.
const FILE_DESCRIPTION: &str = "OP2Lua mission name placeholder - set by the new-mission tool";
/// The stub's DescBlock: Colony (-1), 2 players, tech level 12; its fourth field, the unit-mission flag, follows.
const DESC_BLOCK: [u8; 12] = [0xFF, 0xFF, 0xFF, 0xFF, 0x02, 0, 0, 0, 0x0C, 0, 0, 0];

/// Where `needle` first is in `buf`.
fn find(buf: &[u8], needle: &[u8]) -> Option<usize> {
    buf.windows(needle.len()).position(|w| w == needle)
}

/// The placeholder text `old` in the stub replaced by `new` (under 1023 bytes), zero-ended, the rest of the old text
/// zeroed.
fn patch_text(buf: &mut [u8], old: &str, new: &str) -> Result<()> {
    if new.len() + 1 >= 1024 {
        bail!("{new:?} is too long for the stub");
    }
    let i = find(buf, old.as_bytes()).with_context(|| format!("{old:?} isn't in the stub DLL"))?;
    let clear = old.len().max(new.len() + 1);
    buf[i..i + clear].fill(0);
    buf[i..i + new.len()].copy_from_slice(new.as_bytes());
    Ok(())
}

/// A copy of OP2Lua's stub DLL with the mission's menu name, map, tech tree, type and player count, as
/// `op2lua-newmission --type --players` makes it, whether it's a unit mission (no base: the game greys the command
/// pane's base buttons and hides the stores, as in Tutorial 2), and the computer players it asks for (`DescBlockEx`).
#[allow(clippy::too_many_arguments)]
pub fn stub(
    template: &[u8],
    name: &str,
    map: &str,
    tech: &str,
    mission_type: i32,
    players: i32,
    unit_mission: bool,
    ai_players: i32,
) -> Result<Vec<u8>> {
    let mut buf = template.to_vec();
    patch_text(&mut buf, LEVEL_DESC, name)?;
    patch_text(&mut buf, MAP_NAME, map)?;
    if !tech.eq_ignore_ascii_case(TECH_TREE) {
        patch_text(&mut buf, TECH_TREE, tech)?;
    }
    let wide = |s: &str| -> Vec<u8> { s.bytes().flat_map(|b| [b, 0]).collect() };
    if let Some(i) = find(&buf, &wide(FILE_DESCRIPTION)) {
        let len = FILE_DESCRIPTION.len() * 2;
        let value: String = name.chars().filter(char::is_ascii).take(FILE_DESCRIPTION.len()).collect();
        buf[i..i + len].fill(0);
        buf[i..i + value.len() * 2].copy_from_slice(&wide(&value));
    }
    // DescBlock: mission type (i32 at 0), player count (low byte of the i32 at 4), unit-mission flag (i32 at 12).
    let i = find(&buf, &DESC_BLOCK).context("the stub's DescBlock isn't the template's (Colony, 2 players)")?;
    buf[i..i + 4].copy_from_slice(&mission_type.to_le_bytes());
    buf[i + 4] = players as u8;
    buf[i + 12..i + 16].copy_from_slice(&i32::from(unit_mission).to_le_bytes());
    if ai_players > 0 {
        let at = desc_block_ex(&buf).context("the stub's DescBlockEx isn't in its file (a stub built some other way?)")?;
        buf[at..at + 4].copy_from_slice(&ai_players.to_le_bytes());
    }
    Ok(buf)
}

/// Where the stub's `DescBlockEx` (its first field: the computer players) is in the file, when the file holds it
/// (zero, as the template's is) rather than leaving it to the loader's zero fill.
fn desc_block_ex(buf: &[u8]) -> Option<usize> {
    let pe = op2_x86::pe::PeFile::parse(buf).ok()?;
    let rva = pe.exports.iter().find(|e| e.name.as_deref() == Some("DescBlockEx"))?.rva;
    let s = pe.sections.iter().find(|s| s.contains(rva))?;
    let off = rva - s.rva;
    // The block's 32 bytes must lie in the section's file data, and its first field still be zero.
    (off + 32 <= s.raw_size).then_some((s.raw_offset + off) as usize).filter(|&at| buf.get(at..at + 4) == Some(&[0, 0, 0, 0]))
}
