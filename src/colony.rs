//! `op2dll lua` for a colony game (`CEP1`, `CES1`, `CPP1`, `CPS1`, `CPS2`), a tutorial (`t01` to `t14`) or a
//! campaign mission (a positive mission type, e.g. a community campaign's): its starting placement, from what its
//! setup makes at each difficulty over many seeds, and its win. A campaign mission's carry-over isn't modelled: what a
//! setup reads from the previous mission (a file beside the DLL, as some community campaigns do) is what the run sees.
//!
//! A colony game or tutorial has no lobby: the player picks Easy, Normal or Hard, and the setup reads it off player 1
//! (`_Player::Difficulty`). Its random rolls (`TethysGame::GetRand`) follow the seed. The setup runs at each
//! difficulty until more seeds show nothing new, and what it makes is sorted:
//! - units the setup asks for (`CreateUnit`, the tiles it names) on the same tile in every game of a difficulty are
//!   that difficulty's; a unit on a tile the setup rolled in a rectangle (`MAP_RECT::RandPt` just before) is made
//!   anywhere in it (the computer player's tanks in CES1 and CPS2); a player's other units that all move together
//!   from game to game are a kit on a rolled start (CPS2 rolls one of many spots for the whole colony). Units no
//!   `CreateUnit` made (a `CreateUnitBlock`'s base) are taken as they stand on the map;
//! - beacons by the order the setup makes them: one tile in every game is a fixed beacon; consecutive beacons
//!   drawn from one list of tiles are that many spots picked from it; a beacon whose tiles fill a rectangle is made
//!   anywhere in it (CEP1 rolls its x and y in a box);
//! - what every difficulty makes alike (units, tubes and walls, fixed beacons) goes in placement.lua; the rest, and
//!   each player's colonists, stores, research and Structure Factory kits, go in `<Base>.lua` by difficulty (once,
//!   when every difficulty makes the same).
//!
//! The win is `victory.rs`'s and the disasters and other timed triggers `timed.rs`'s, each difficulty's merged into one
//! `on_init` with `if`s where they differ. A computer player the setup gives AI groups (or, in a campaign mission
//! with two players or more, one whose groups only its trigger functions make) is `ai.rs`'s: its base as
//! `setup_ai`, its trigger functions as Lua. Triggers, the base, and the beacons, wreckage, markers and tubes made
//! among them are made in the order the setup makes them, since the game numbers them so. Triggers neither converts
//! and an `AIProc` that makes game calls are listed at the end of the Lua.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result, bail};
use op2_missionhost::runtime::{CallRec, Options};

use crate::luaexport::{
    Beacon, DIRECTIONS, LEVEL_NAMES, Level, Lua, Run, Unit, at, cargo_name, created, int, last_arg, lava_rects, level_of, lua_name, morale_calls,
    quoted, seed,
};

/// Games run at each difficulty: at least `MIN_GAMES`, then until `QUIET` games in a row show nothing new.
const MIN_GAMES: u32 = 64;
/// Games in a row showing no new unit or beacon tile that end a difficulty's runs.
const QUIET: u32 = 150;
/// The most games run at one difficulty, whatever they show.
const MAX_GAMES: u32 = 3000;
/// A list of beacon tiles is an area when it has at least this many tiles and fills half its rectangle.
const AREA_MIN: usize = 8;
/// Marks of a replay with nobody playing, to see whether AIProc does anything.
const REPLAY_MARKS: u32 = 300;
/// OP2Lua's player colours, by the game's colour number.
const COLORS: [&str; 8] = ["Blue", "Red", "Green", "Yellow", "Cyan", "Magenta", "Black", "White"];

/// One run of the setup with `seed` at `difficulty` (0 Easy, 1 Normal, 2 Hard), its calls logged; an error when the
/// setup stops.
fn run(dll: &Path, seed: u32, difficulty: i32) -> Result<Run> {
    let opts = Options { seed: u64::from(seed), difficulty, call_log: true, ..Options::from_env() };
    let (setup, calls) = crate::setup::run_with(dll, &opts)?;
    if let Some(f) = &setup.failed {
        bail!("the setup stopped: {f}");
    }
    Ok(Run { players: setup.players.len() as u8, setup, calls })
}

/// The setup's (`InitProc`'s) calls of `api`, each as its arguments split at every comma.
fn init_calls<'a>(calls: &'a [CallRec], api: &'a str) -> impl Iterator<Item = Vec<&'a str>> + 'a {
    calls
        .iter()
        .filter(move |c| c.api == api && c.phase == "InitProc")
        .map(|c| c.args.iter().flat_map(|a| a.split(',')).map(str::trim).collect())
}

/// The beacons the setup asks for, in order (file tiles): `CreateBeacon(type, x, y, ore, yield, variant)`.
fn beacons_made(calls: &[CallRec], pad: i32) -> Vec<((i32, i32), Beacon)> {
    init_calls(calls, "TethysGame::CreateBeacon")
        .filter_map(|a| {
            let [ty, x, y, ore, bar, variant] = a[..] else { return None };
            let b = Beacon { type_name: ty.to_string(), ore: int(ore)? as i64, bar: int(bar)? as i64, variant: int(variant)? as i64 };
            Some(((int(x)? - pad, int(y)?), b))
        })
        .collect()
}

/// The tubes and walls the setup asks for (file tiles): `CreateWallOrTube(x, y, _, type)`.
fn walls_made(calls: &[CallRec], pad: i32) -> BTreeSet<((i32, i32), String)> {
    init_calls(calls, "TethysGame::CreateWallOrTube")
        .filter_map(|a| {
            let [x, y, _, ty] = a[..] else { return None };
            Some(((int(x)? - pad, int(y)?), ty.to_string()))
        })
        .collect()
}

/// A tube or wall: its tile (file tiles) and the game's type name.
type Wall = ((i32, i32), String);

/// A wreck: the draw (size, value) just before it, its tile (file tiles), the tech and whether it's discovered.
type Wreck = (Option<(i64, i64)>, (i32, i32), i32, bool);

/// A run of the setup's tubes and walls with nothing else made among them: the trigger or group made last before it
/// (0 for none), and its tubes and walls in order.
struct WallRun {
    /// The number of the trigger or group made last before the run (0 for none).
    stub: u32,
    /// Its tubes and walls in the order made.
    walls: Vec<((i32, i32), String)>,
    /// Its tubes and walls on a tile a map edit before it changed.
    over_edits: bool,
}

/// The setup's tubes and walls in runs, as it makes them (Normal's first game is what the caller passes).
fn wall_runs(calls: &[CallRec], pad: i32) -> Vec<WallRun> {
    let mut runs: Vec<WallRun> = Vec::new();
    let (mut stub, mut open) = (0, false);
    let mut edited: BTreeSet<(i32, i32)> = BTreeSet::new();
    for c in calls.iter().filter(|c| c.phase == "InitProc" && c.api != "ScStub::~ScStub") {
        let a: Vec<&str> = c.args.iter().flat_map(|a| a.split(',')).map(|s| s.trim().trim_matches(|ch| ch == '(' || ch == ')')).collect();
        if c.api == "TethysGame::CreateWallOrTube" {
            let [x, y, _, ty] = a[..] else { continue };
            let (Some(x), Some(y)) = (int(x), int(y)) else { continue };
            let tile = (x - pad, y);
            if !open {
                runs.push(WallRun { stub, walls: Vec::new(), over_edits: false });
                open = true;
            }
            let run = runs.last_mut().expect("a run is open");
            run.over_edits |= edited.contains(&tile);
            run.walls.push((tile, ty.to_string()));
            continue;
        }
        let edit = EDITS.iter().any(|(api, _)| *api == c.api);
        if c.api.starts_with("Create") && (c.api.ends_with("Trigger") || c.api.ends_with("Condition") || c.api.ends_with("Group")) {
            stub = c.ret.rsplit('#').next().and_then(|n| n.trim().parse().ok()).unwrap_or(stub);
        } else if edit
            && let [x, y, ..] = a[..]
            && let (Some(x), Some(y)) = (int(x), int(y))
        {
            edited.insert((x - pad, y));
        }
        // Only a call that makes something (a unit, beacon, marker, trigger or group) or edits the map ends a run.
        if c.api.starts_with("TethysGame::Create") || c.api == "TethysGame::PlaceMarker" || c.api.starts_with("Create") || edit {
            open = false;
        }
    }
    runs
}

/// OP2Lua's type name of a tube or wall (anything else is a tube).
fn wall_type(w: &str) -> &'static str {
    match w {
        "WALL" => "Wall",
        "WALL_LAVA" => "LavaWall",
        "WALL_MICROBE" => "MicrobeWall",
        _ => "Tube",
    }
}

/// A `make_walls` call of these tubes and walls, in order, six to a line.
fn walls_lua(walls: &[((i32, i32), String)]) -> String {
    let items: Vec<String> = walls.iter().map(|(t, w)| format!("{{ \"{}\", {} }}", wall_type(w), at(*t))).collect();
    let mut out = String::from("  make_walls({
");
    for c in items.chunks(6) {
        let _ = writeln!(out, "    {},", c.join(", "));
    }
    out.push_str("  })
");
    out
}

/// The map edits a setup makes (Tutorial 5's), by the game's names and OP2Lua's.
const EDITS: [(&str, &str); 3] =
    [("GameMap::SetTile", "set_tile"), ("GameMap::SetCellType", "set_cell_type"), ("GameMap::InitialSetTile", "initial_set_tile")];

/// One of the setup's map edits: which (`EDITS`), its tile, its number, and the draw (size, value) made just before
/// it, if one was.
#[derive(Clone)]
struct MapEdit {
    /// Its index in `EDITS`.
    kind: usize,
    /// File tiles.
    tile: (i32, i32),
    /// The call's last argument: the tile mapping or cell type set.
    value: i64,
    /// The `GetRand` just before it: its range and the value drawn.
    draw: Option<(i64, i64)>,
}

/// The setup's map edits, in order, each with the `GetRand` made right before it (no other call between).
fn edits_made(calls: &[CallRec], pad: i32) -> Vec<MapEdit> {
    let mut out = Vec::new();
    let mut draw = None;
    for c in calls.iter().filter(|c| c.phase == "InitProc" && c.api != "ScStub::~ScStub") {
        let a: Vec<&str> = c.args.iter().flat_map(|a| a.split(',')).map(|s| s.trim().trim_matches(|ch| ch == '(' || ch == ')')).collect();
        if c.api == "TethysGame::GetRand" {
            draw = a.first().and_then(|n| int(n)).zip(int(c.ret.trim())).map(|(n, v)| (n as i64, v as i64));
            continue;
        }
        let kind = EDITS.iter().position(|(api, _)| *api == c.api);
        if let (Some(kind), [x, y, v]) = (kind, &a[..])
            && let (Some(x), Some(y), Some(v)) = (int(x), int(y), int(v))
        {
            out.push(MapEdit { kind, tile: (x - pad, y), value: v as i64, draw });
        }
        draw = None;
    }
    out
}

/// The Lua of the map edits `games` made, the same calls on the same tiles in each, with each number fitted to the
/// draw before it; or why not. A drawn edit makes the same `game.rand` first, so later draws follow the original's.
fn edits_lua(games: &[Game]) -> Result<Vec<String>, String> {
    let first = &games[0].edits;
    let shape = |e: &MapEdit| (e.kind, e.tile, e.draw.map(|d| d.0));
    if games.iter().any(|g| g.edits.len() != first.len() || g.edits.iter().zip(first).any(|(a, b)| shape(a) != shape(b))) {
        return Err("the setup's map edits change from game to game".into());
    }
    let mut lines = Vec::new();
    for (k, e) in first.iter().enumerate() {
        let call = |value: String| format!("game.{}({}, {value})", EDITS[e.kind].1, at(e.tile));
        match e.draw {
            None => {
                if games.iter().any(|g| g.edits[k].value != e.value) {
                    return Err(format!("map edit {} changes from game to game with no draw before it", k + 1));
                }
                lines.push(call(e.value.to_string()));
            }
            Some((n, _)) => {
                let draws: Vec<[i64; 1]> = games.iter().map(|g| [g.edits[k].draw.map_or(0, |d| d.1)]).collect();
                let samples: Vec<(&[i64], i64)> = draws.iter().zip(games).map(|(d, g)| (&d[..], g.edits[k].value)).collect();
                let expr = crate::timed::fit(&samples, &[n]).ok_or_else(|| format!("map edit {} follows no rule of its draw", k + 1))?;
                let value = expr.lua(&["r".to_string()]);
                lines.push(if value.contains('r') { format!("r = game.rand({n}); {}", call(value)) } else { format!("game.rand({n}); {}", call(value)) });
            }
        }
    }
    Ok(lines)
}

/// Beacons as made: the tile and the kind.
type Beacons = Vec<((i32, i32), Beacon)>;

/// A unit with the id of the object it became (its place in the order of creation) and the rectangle it was
/// rolled in.
type Placed = (u32, Unit, Option<[i32; 4]>);

/// The tile of `(x, y)` or `x, y`.
fn tile_of(s: &str) -> Option<(i32, i32)> {
    let (x, y) = s.trim().trim_start_matches('(').trim_end_matches(')').split_once(',')?;
    Some((int(x)?, int(y)?))
}

/// What a game made: the units the setup asked for (`created`) with the rectangle each was rolled in, then the
/// units on the map no `CreateUnit` made (a `CreateUnitBlock`'s), as they stand.
fn game_of(r: &Run, pad: i32) -> Game {
    let mut units = created(r, pad);
    // By owner, for each CreateUnit in order, the rectangle its tile was rolled in: a RandPt's whose point it is.
    let mut areas: BTreeMap<i32, Vec<Option<[i32; 4]>>> = BTreeMap::new();
    let mut last: Option<([i32; 4], (i32, i32))> = None;
    for c in r.calls.iter().filter(|c| c.phase == "InitProc") {
        match c.api.as_str() {
            "MAP_RECT::RandPt" => {
                let rect = c.args.join(",");
                let rect = rect.trim_start_matches("this=[").trim_end_matches(']');
                last = rect.split_once("..").and_then(|(a, b)| {
                    let ((x1, y1), (x2, y2)) = (tile_of(a)?, tile_of(b)?);
                    Some(([x1 - pad, y1, x2 - pad, y2], tile_of(&c.ret)?))
                });
            }
            "TethysGame::CreateUnit" => {
                let a: Vec<&str> = c.args.iter().flat_map(|a| a.split(',')).map(str::trim).collect();
                let [_, _, x, y, owner, _, _] = a[..] else { continue };
                let (Some(x), Some(y), Some(owner)) = (int(x.trim_start_matches('(')), int(y.trim_end_matches(')')), int(owner)) else {
                    continue;
                };
                let area = last.take().filter(|(_, t)| *t == (x, y)).map(|(rect, _)| rect);
                areas.entry(owner).or_default().push(area);
            }
            _ => {}
        }
    }
    // More of an owner's type on the map than were asked for: the extra ones are a block's, unless they stand where
    // the owner asked for one of that type, or for a Common Ore Mine (the game makes one on a rare beacon a Rare Ore
    // Mine, on a magma vent a Magma Well). A unit asked for on a taken tile stands elsewhere, so the counts go by type.
    let mine = crate::luaexport::map_id("MINE");
    let mines = [mine, crate::luaexport::map_id("MINE_ADV"), crate::luaexport::map_id("MAGMA_WELL")];
    let mut made: BTreeMap<(i32, i32), usize> = BTreeMap::new();
    let mut asked: BTreeSet<(i32, i32, (i32, i32))> = BTreeSet::new();
    for (owner, us) in &units {
        for u in us {
            *made.entry((*owner, u.type_id)).or_default() += 1;
            asked.insert((*owner, u.type_id, u.tile));
        }
    }
    let asked_for = |o: &crate::setup::Object| {
        asked.contains(&(o.owner, o.type_id, o.tile)) || (mines.contains(&o.type_id) && asked.contains(&(o.owner, mine, o.tile)))
    };
    let mut on_map: Vec<&crate::setup::Object> =
        r.setup.objects.iter().filter(|o| matches!(o.kind, crate::setup::Kind::Vehicle | crate::setup::Kind::Structure)).collect();
    on_map.sort_by_key(|o| o.id);
    let mut extra: BTreeMap<(i32, i32), usize> = BTreeMap::new();
    for o in &on_map {
        *extra.entry((o.owner, o.type_id)).or_default() += 1;
    }
    for (k, n) in extra.iter_mut() {
        *n = n.saturating_sub(made.get(k).copied().unwrap_or(0));
    }
    let mut block: BTreeSet<u32> = BTreeSet::new();
    for o in &on_map {
        if let Some(n) = extra.get_mut(&(o.owner, o.type_id)).filter(|n| **n > 0 && !asked_for(o)) {
            *n -= 1;
            block.insert(o.id);
        }
    }
    // Every unit in the order the game made it (object ids): it numbers units so and moves them in that order each
    // tick, so a unit block made before a CreateUnit stays before it. A unit asked for takes the place of the object
    // it became, matched by owner and type in order (mines as mines); its lights are that object's.
    let as_mine = |t: i32| if mines.contains(&t) { mine } else { t };
    let mut objects: BTreeMap<(i32, i32), std::collections::VecDeque<&crate::setup::Object>> = BTreeMap::new();
    for o in on_map.iter().filter(|o| !block.contains(&o.id)) {
        objects.entry((o.owner, as_mine(o.type_id))).or_default().push_back(o);
    }
    let mut ordered: BTreeMap<i32, Vec<Placed>> = BTreeMap::new();
    for (owner, us) in std::mem::take(&mut units) {
        let rolled = areas.remove(&owner).unwrap_or_default();
        for (i, u) in us.into_iter().enumerate() {
            let o = objects.get_mut(&(owner, as_mine(u.type_id))).and_then(|q| q.pop_front());
            let lights = o.is_some_and(|o| o.lights);
            let id = o.map_or(u32::MAX, |o| o.id);
            ordered.entry(owner).or_default().push((id, Unit { lights, ..u }, rolled.get(i).copied().flatten()));
        }
    }
    for o in on_map.iter().filter(|o| block.contains(&o.id)) {
        let e = |k: &str| o.extra.get(k).copied().unwrap_or(0) as i32;
        let cargo = (e("truckCargo"), e("truckAmount"));
        // A block's mine is written as the Common Ore Mine the stock setups ask for: the game makes it a Rare Ore Mine
        // or a Magma Well from the beacon under it, and `CreateUnit` of a Rare Ore Mine itself crashes the game
        // (Tutorial P's at 60,60, in OPU 1.4.1).
        let unit = Unit {
            vehicle: matches!(o.kind, crate::setup::Kind::Vehicle),
            type_id: as_mine(o.type_id),
            tile: o.tile,
            rotation: o.rotation.rem_euclid(8),
            weapon: o.weapon_id.max(0),
            cargo: if cargo.0 > 0 { cargo } else { (0, 0) },
            topping: if matches!(o.kind, crate::setup::Kind::Vehicle) { e("cargoWeapon").max(0) } else { 0 },
            lights: o.lights,
            garage: None,
        };
        ordered.entry(o.owner).or_default().push((o.id, unit, None));
    }
    let mut sequence: Vec<(u32, Made)> = Vec::new();
    for (owner, mut list) in ordered {
        list.sort_by_key(|(id, _, _)| *id);
        sequence.extend(list.iter().enumerate().map(|(i, (id, _, _))| (*id, Made::Unit(owner, i))));
        let (us, rolled): (Vec<Unit>, Vec<Option<[i32; 4]>>) = list.into_iter().map(|(_, u, a)| (u, a)).unzip();
        units.insert(owner, us);
        areas.insert(owner, rolled);
    }
    sequence.extend(r.setup.objects.iter().filter(|o| matches!(o.kind, crate::setup::Kind::Resource)).map(|o| (o.id, Made::Beacon(o.tile))));
    sequence.sort_by_key(|(id, _)| *id);
    let sequence = sequence.into_iter().map(|(_, m)| m).collect();
    // How many units the game made before each beacon (OP2Lua's placement makes a beacon after that many of its units,
    // so the units keep the original's ids), when the placement's order (by player, then as made) is the order made.
    let by_player = on_map.windows(2).all(|w| w[0].owner <= w[1].owner);
    let after = r
        .setup
        .objects
        .iter()
        .filter(|o| by_player && matches!(o.kind, crate::setup::Kind::Resource))
        .map(|b| (b.tile, on_map.iter().filter(|o| o.id < b.id).count()))
        .collect();
    // The draws since the last other call, taken by the beacon or wreck they come before.
    let mut beacon_draws = Vec::new();
    let mut wrecks = Vec::new();
    let mut pending = Vec::new();
    for c in r.calls.iter().filter(|c| c.phase == "InitProc" && c.api != "ScStub::~ScStub") {
        match c.api.as_str() {
            "TethysGame::GetRand" => {
                if let (Some(n), Some(v)) = (c.args.first().and_then(|a| int(a.trim())), int(c.ret.trim())) {
                    pending.push((n as i64, v as i64));
                }
            }
            "TethysGame::CreateBeacon" => beacon_draws.push(std::mem::take(&mut pending)),
            // "x, y, map_id(tech), discovered"
            "TethysGame::CreateWreck" => {
                let a: Vec<String> = c.args.iter().flat_map(|a| a.split(',')).map(|v| v.trim().to_string()).collect();
                let tech = a.get(2).and_then(|t| t.strip_prefix("map_id(")).and_then(|t| t.strip_suffix(')')).and_then(|t| t.parse::<i32>().ok());
                if let ([x, y, _, discovered, ..], Some(tech)) = (&a[..], tech)
                    && let (Some(x), Some(y)) = (int(x), int(y))
                {
                    wrecks.push((pending.last().copied(), (x - pad, y), tech, discovered.trim() != "0"));
                }
                pending.clear();
            }
            _ => pending.clear(),
        }
    }
    Game {
        units,
        areas,
        walls: walls_made(&r.calls, pad),
        beacons: beacons_made(&r.calls, pad),
        after,
        beacon_draws,
        sequence,
        edits: edits_made(&r.calls, pad),
        wrecks,
    }
}

/// The Lua of the wreckage `games` left (Eden 9's four: each tile drawn from a table by a GetRand just before it; the
/// table learned from the draws). Each wreck the same in every game, or on a tile its draw picks.
fn wrecks_lua(games: &[Game]) -> Result<String, String> {
    let first = &games.first().ok_or("no games")?.wrecks;
    if games.iter().any(|g| g.wrecks.len() != first.len()) {
        return Err("the number of wrecks differs from game to game".into());
    }
    let mut out = String::new();
    for (k, (draw, tile, tech, seen)) in first.iter().enumerate() {
        let seen = if *seen { ", true" } else { "" };
        if games.iter().all(|g| g.wrecks[k].1 == *tile && g.wrecks[k].2 == *tech) {
            let _ = writeln!(out, "  game.create_wreckage({}, {tech}{seen})", at(*tile));
            continue;
        }
        let Some((n, _)) = draw else { return Err(format!("wreck {} moves with no draw before it", k + 1)) };
        let mut table: BTreeMap<i64, (i32, i32)> = BTreeMap::new();
        for g in games {
            let (d, t, te, _) = &g.wrecks[k];
            let Some((dn, dv)) = d.filter(|(dn, _)| dn == n) else { return Err(format!("wreck {} without its draw of {n}", k + 1)) };
            if *te != *tech || *table.entry(dv).or_insert(*t) != *t || dn != *n {
                return Err(format!("wreck {}'s tile isn't one for each draw", k + 1));
            }
        }
        if table.len() as i64 != *n {
            return Err(format!("wreck {}: {} of its {n} tiles seen", k + 1, table.len()));
        }
        let spots: Vec<String> = table.values().map(|t| at(*t)).collect();
        let _ = writeln!(out, "  game.create_wreckage(({{ {} }})[game.rand({n}) + 1], {tech}{seen})", spots.join(", "));
    }
    Ok(out)
}

/// What one game made.
struct Game {
    /// By player, its units in the order the game made them.
    units: BTreeMap<i32, Vec<Unit>>,
    /// For each unit, the rectangle (file tiles, inclusive) the setup rolled its tile in (`MAP_RECT::RandPt`).
    areas: BTreeMap<i32, Vec<Option<[i32; 4]>>>,
    /// The tubes and walls the setup asked for.
    walls: BTreeSet<((i32, i32), String)>,
    /// The beacons the setup asked for, in order.
    beacons: Vec<((i32, i32), Beacon)>,
    /// By beacon tile, how many units the game made before it (none when the units weren't made player by player).
    after: BTreeMap<(i32, i32), usize>,
    /// For each beacon, the draws (size, value) the setup made since its last other call before it.
    beacon_draws: Vec<Vec<(i64, i64)>>,
    /// The units (by player and place in `units`) and beacons, in the order the game made them.
    sequence: Vec<Made>,
    /// The setup's map edits, in order.
    edits: Vec<MapEdit>,
    /// The wreckage the setup left, in order: the draw (size, value) just before each, its tile (file tiles), the
    /// tech and whether it's discovered.
    wrecks: Vec<Wreck>,
}

/// One of the things a setup made: a player's unit (its place in [`Game::units`]) or a beacon (its tile).
#[derive(Clone, PartialEq)]
enum Made {
    /// A unit: its player and place in that player's list.
    Unit(i32, usize),
    /// A beacon: its tile.
    Beacon((i32, i32)),
}

/// A player's units at one difficulty: on fixed tiles, each on a tile rolled in a rectangle, and a kit placed
/// relative to a start rolled from `starts`.
#[derive(Default)]
struct Units {
    /// Units on the same tile in every game.
    fixed: Vec<Unit>,
    /// Units each rolled in its rectangle (file tiles, inclusive).
    rolled: Vec<(Unit, [i32; 4])>,
    /// The kit's start tiles seen (its first unit's tile).
    starts: Vec<(i32, i32)>,
    /// Units that move together, tiles relative to the start.
    kit: Vec<Unit>,
    /// The camera's tile, relative to the start when there is one.
    view: Option<(i32, i32)>,
}

/// Beacons made a random way.
#[derive(Clone, PartialEq)]
enum Rolled {
    /// `count` of `spots` (no two alike), with the kinds made on them, by pick. With `table`, the spots in the order of
    /// the DLL's table: the setup draws an index into it, again while it repeats one, all picks before any beacon.
    Spots { count: usize, kinds: Vec<Beacon>, spots: Vec<(i32, i32)>, table: Option<Vec<(i32, i32)>> },
    /// Anywhere in the rectangle (file tiles, inclusive). With `draws`, as the setup draws it: two numbers of these
    /// sizes, the `x_draw`-th (0 or 1) added to `base`'s x and the other to its y.
    Area { kind: Beacon, rect: [i32; 4], draws: Option<([i64; 2], usize, (i32, i32))> },
    /// On a fixed tile, one of these kinds.
    Kinds { tile: (i32, i32), kinds: Vec<Beacon> },
}

/// What one difficulty makes.
struct Difficulty {
    /// By player, its units and its colonists, stores, research and kits.
    players: BTreeMap<i32, (Units, Level)>,
    /// The first game's tubes and walls.
    walls: BTreeSet<((i32, i32), String)>,
    /// The beacons on a fixed tile and of a fixed kind.
    beacons: Vec<((i32, i32), Beacon)>,
    /// The beacons made a random way, in the order made.
    rolled: Vec<Rolled>,
    /// By beacon tile, how many units every game made before it.
    after: BTreeMap<(i32, i32), usize>,
    /// The units the first game made.
    units_made: usize,
    /// What the first game made, in order.
    sequence: Vec<Made>,
    /// The Lua of the setup's map edits.
    edits: Result<Vec<String>, String>,
    /// The Lua of the wreckage the setup leaves.
    wrecks: Result<String, String>,
    /// How many games ran.
    games: u32,
}

/// Multiset difference: the units of `all` left after taking out one of each in `common`.
fn without(all: &[Unit], common: &[Unit]) -> Vec<Unit> {
    let mut left: BTreeMap<&Unit, usize> = BTreeMap::new();
    for u in common {
        *left.entry(u).or_default() += 1;
    }
    all.iter()
        .filter(|u| match left.get_mut(u) {
            Some(n) if *n > 0 => {
                *n -= 1;
                false
            }
            _ => true,
        })
        .cloned()
        .collect()
}

/// The units in every one of `lists`, in the first list's order.
fn common_units(lists: &[&[Unit]]) -> Vec<Unit> {
    let Some((first, rest)) = lists.split_first() else { return Vec::new() };
    let mut counts: Vec<BTreeMap<&Unit, usize>> = rest
        .iter()
        .map(|l| {
            let mut m = BTreeMap::new();
            for u in l.iter() {
                *m.entry(u).or_default() += 1;
            }
            m
        })
        .collect();
    first
        .iter()
        .filter(|u| {
            if counts.iter().all(|m| m.get(u).is_some_and(|&n| n > 0)) {
                counts.iter_mut().for_each(|m| *m.get_mut(u).unwrap() -= 1);
                true
            } else {
                false
            }
        })
        .cloned()
        .collect()
}

/// Player `player`'s units over `games` (one difficulty's, named `level` in notes), sorted into fixed, rolled and a kit
/// on a rolled start, with the camera tile `view` relative to the start when there is one. When the games differ in
/// number or type, or the moving units don't move together, the first game's are all fixed and a note says so.
fn units_of(games: &[Game], player: i32, view: Option<(i32, i32)>, notes: &mut Vec<String>, level: &str) -> Units {
    let empty = Vec::new();
    let lists: Vec<&Vec<Unit>> = games.iter().map(|g| g.units.get(&player).unwrap_or(&empty)).collect();
    let first = lists[0];
    let same_shape = lists.iter().all(|l| l.len() == first.len() && l.iter().zip(first.iter()).all(|(a, b)| a.type_id == b.type_id));
    if !same_shape {
        notes.push(format!("{level}: player {}'s units differ from game to game in number or type (the first game's are used)", player + 1));
        return Units { fixed: first.clone(), view, ..Default::default() };
    }
    // Units each rolled in one rectangle in every game.
    let no_areas = Vec::new();
    let areas: Vec<&Vec<Option<[i32; 4]>>> = games.iter().map(|g| g.areas.get(&player).unwrap_or(&no_areas)).collect();
    let area_of = |i: usize| -> Option<[i32; 4]> {
        let a = areas[0].get(i).copied().flatten()?;
        areas.iter().all(|x| x.get(i).copied().flatten() == Some(a)).then_some(a)
    };
    let mut rolled = Vec::new();
    for i in 0..first.len() {
        if let Some(a) = area_of(i) {
            let outside = |t: (i32, i32)| t.0 < a[0] || t.0 > a[2] || t.1 < a[1] || t.1 > a[3];
            if lists.iter().any(|l| outside(l[i].tile)) {
                notes.push(format!("{level}: a unit of player {} stood outside the rectangle it was rolled in", player + 1));
            }
            rolled.push((first[i].clone(), a));
        }
    }
    // The rest: on one tile in every game, or moving; moving units are a kit when each game moves them all by one
    // offset, the start being where the first of them stands.
    let moving: Vec<usize> =
        (0..first.len()).filter(|&i| area_of(i).is_none() && lists.iter().any(|l| l[i].tile != first[i].tile)).collect();
    let fixed: Vec<Unit> = (0..first.len()).filter(|&i| area_of(i).is_none() && !moving.contains(&i)).map(|i| first[i].clone()).collect();
    if moving.is_empty() {
        return Units { fixed, rolled, view, ..Default::default() };
    }
    let anchor = first[moving[0]].tile;
    let mut starts = BTreeSet::new();
    for l in &lists {
        let deltas: BTreeSet<(i32, i32)> = moving.iter().map(|&i| (l[i].tile.0 - first[i].tile.0, l[i].tile.1 - first[i].tile.1)).collect();
        if deltas.len() != 1 {
            notes.push(format!("{level}: player {}'s units move from game to game, not together (the first game's are used)", player + 1));
            let fixed = (0..first.len()).filter(|&i| area_of(i).is_none()).map(|i| first[i].clone()).collect();
            return Units { fixed, rolled, view, ..Default::default() };
        }
        let d = deltas.into_iter().next().unwrap_or_default();
        starts.insert((anchor.0 + d.0, anchor.1 + d.1));
    }
    let kit = moving.iter().map(|&i| Unit { tile: (first[i].tile.0 - anchor.0, first[i].tile.1 - anchor.1), ..first[i].clone() }).collect();
    Units { fixed, rolled, starts: starts.into_iter().collect(), kit, view: view.map(|v| (v.0 - anchor.0, v.1 - anchor.1)) }
}

/// The DLL's table of spots for beacons `i..j`, when every game drew their indices before beacon `i` (one size, again
/// on a repeat, one distinct value per beacon, in order) and each index always meant the same tile, every index seen.
fn spot_table(games: &[Game], i: usize, j: usize) -> Option<Vec<(i32, i32)>> {
    let size = games.first()?.beacon_draws.get(i)?.first()?.0;
    let mut table: BTreeMap<i64, (i32, i32)> = BTreeMap::new();
    for g in games {
        let draws = g.beacon_draws.get(i)?;
        if draws.iter().any(|d| d.0 != size) || (i + 1..j).any(|k| g.beacon_draws.get(k).is_none_or(|d| !d.is_empty())) {
            return None;
        }
        let mut picks: Vec<i64> = Vec::new();
        for d in draws {
            if !picks.contains(&d.1) {
                picks.push(d.1);
            }
        }
        if picks.len() != j - i {
            return None;
        }
        for (k, v) in picks.iter().enumerate() {
            let tile = g.beacons[i + k].0;
            if *table.entry(*v).or_insert(tile) != tile {
                return None;
            }
        }
    }
    (table.len() as i64 == size).then(|| table.into_values().collect())
}

/// How the setup draws beacon `k`'s tile: two draws of fixed sizes just before it, one added to a fixed x, the other
/// to a fixed y, the same in every game.
fn area_draws(games: &[Game], k: usize) -> Option<([i64; 2], usize, (i32, i32))> {
    let first = games.first()?.beacon_draws.get(k)?;
    let [a, b] = first[..] else { return None };
    let sizes = [a.0, b.0];
    (0..2).find_map(|x| {
        let base = |g: &Game| -> Option<(i32, i32)> {
            let d = g.beacon_draws.get(k)?;
            if d.len() != 2 || d[0].0 != sizes[0] || d[1].0 != sizes[1] {
                return None;
            }
            let t = g.beacons.get(k)?.0;
            Some((t.0 - d[x].1 as i32, t.1 - d[1 - x].1 as i32))
        };
        let b0 = base(&games[0])?;
        games.iter().all(|g| base(g) == Some(b0)).then_some((sizes, x, b0))
    })
}

/// Fixed beacons and rolled ones, from the beacons each game made, by the order they were made. Consecutive beacons
/// with the same set of tiles over the games are one rolled group: an area when the tiles fill it, else spots. When
/// the number of beacons changes, only those in every game are kept (fixed), with a note.
fn beacons_of(games: &[Game], notes: &mut Vec<String>, level: &str) -> (Beacons, Vec<Rolled>) {
    let n = games[0].beacons.len();
    if games.iter().any(|g| g.beacons.len() != n) {
        let every: Vec<((i32, i32), Beacon)> =
            games[0].beacons.iter().filter(|b| games.iter().all(|g| g.beacons.contains(b))).cloned().collect();
        notes.push(format!(
            "{level}: the number of beacons changes from game to game; the {} made in every game are kept, the others aren't converted",
            every.len()
        ));
        return (every, Vec::new());
    }
    let tiles: Vec<BTreeSet<(i32, i32)>> = (0..n).map(|i| games.iter().map(|g| g.beacons[i].0).collect()).collect();
    let kinds: Vec<BTreeSet<Beacon>> = (0..n).map(|i| games.iter().map(|g| g.beacons[i].1.clone()).collect()).collect();
    let (mut fixed, mut rolled) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < n {
        if tiles[i].len() == 1 {
            let tile = *tiles[i].first().unwrap();
            if kinds[i].len() == 1 {
                fixed.push((tile, kinds[i].first().unwrap().clone()));
            } else {
                rolled.push(Rolled::Kinds { tile, kinds: kinds[i].iter().cloned().collect() });
            }
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < n && tiles[j] == tiles[i] {
            j += 1;
        }
        let one_kind = (i..j).all(|k| kinds[k].len() == 1);
        if !one_kind {
            notes.push(format!("{level}: beacons {}-{} change both tile and kind from game to game (the kind of the first game is used)", i + 1, j));
        }
        let picks: Vec<Beacon> = (i..j).map(|k| kinds[k].first().unwrap().clone()).collect();
        let t = &tiles[i];
        let (x1, x2) = (t.iter().map(|p| p.0).min().unwrap(), t.iter().map(|p| p.0).max().unwrap());
        let (y1, y2) = (t.iter().map(|p| p.1).min().unwrap(), t.iter().map(|p| p.1).max().unwrap());
        let area = ((x2 - x1 + 1) * (y2 - y1 + 1)) as usize;
        if t.len() >= AREA_MIN && t.len() * 2 >= area {
            for (k, kind) in picks.into_iter().enumerate() {
                rolled.push(Rolled::Area { kind, rect: [x1, y1, x2, y2], draws: area_draws(games, i + k) });
            }
        } else {
            let table = spot_table(games, i, j);
            rolled.push(Rolled::Spots { count: j - i, kinds: picks, spots: t.iter().copied().collect(), table });
        }
        i = j;
    }
    (fixed, rolled)
}

/// A beacon's fields as OP2Lua's placement and `game.create_beacon` take them.
fn beacon_kind_of(b: &Beacon) -> String {
    let ore = match b.ore { 0 => "Common", 1 => "Rare", _ => "Random" };
    let bar = match b.bar { 0 => "Bar3", 1 => "Bar2", 2 => "Bar1", _ => "Random" };
    let variant = crate::luaexport::variant_name(b.bar, b.variant);
    let kind = match b.type_name.as_str() { "MAGMA_VENT" => "MagmaVent", "FUMAROLE" => "Fumarole", _ => "MiningBeacon" };
    format!(r#"type = "{kind}", ore = "{ore}", yield = "{bar}", variant = "{variant}""#)
}

/// Converts `dll`, a colony game, tutorial or campaign mission, to OP2Lua's placement and mission script; what isn't
/// converted is in the notes. `progress` hears how many games each difficulty ran.
pub fn convert(dll: &Path, report: &crate::inspect::Report, progress: &mut dyn FnMut(&str)) -> Result<Lua> {
    let desc = report.desc.as_ref().with_context(|| format!("{} isn't a mission", dll.display()))?;
    let size = report.map.as_ref().map(|m| (m.width as i32, m.height as i32)).with_context(|| format!("{}: the map wasn't found (--game-dir)", dll.display()))?;
    let pad = if size.0 >= 512 { 0 } else { 32 };
    let mut notes: Vec<String> = Vec::new();

    let mut levels: Vec<Difficulty> = Vec::new();
    let mut firsts: Vec<Run> = Vec::new();
    for difficulty in 0..3 {
        let name = LEVEL_NAMES[difficulty as usize];
        let mut games: Vec<Game> = Vec::new();
        let mut seen: BTreeSet<(i32, usize, (i32, i32))> = BTreeSet::new();
        let mut first: Option<Run> = None;
        let (mut i, mut quiet) = (0, 0);
        while i < MAX_GAMES && (i < MIN_GAMES || quiet < QUIET) {
            let r = run(dll, seed(i), difficulty)?;
            let g = game_of(&r, pad);
            // New: a unit not rolled in a rectangle, or a beacon, on a tile not seen in its place before.
            let mut new = false;
            for (p, us) in &g.units {
                let areas = &g.areas[p];
                for (k, u) in us.iter().enumerate().filter(|(k, _)| areas.get(*k).is_none_or(Option::is_none)) {
                    new |= seen.insert((*p, k, u.tile));
                }
            }
            for (k, (t, _)) in g.beacons.iter().enumerate() {
                new |= seen.insert((-1, k, *t));
            }
            quiet = if new { 0 } else { quiet + 1 };
            games.push(g);
            if first.is_none() {
                first = Some(r);
            }
            i += 1;
        }
        progress(&format!("{name}: {i} games"));
        let first = first.context("no games ran")?;
        let mut players = BTreeMap::new();
        for p in &first.setup.players {
            let view = p.start.view;
            let units = units_of(&games, p.id, view, &mut notes, name);
            // Only the numbers the setup sets (Tutorial 2 sets none).
            let mut level = level_of(&first, p.id);
            level.numbers.retain(|(k, _)| {
                let api = match *k {
                    "workers" => "_Player::SetWorkers",
                    "scientists" => "_Player::SetScientists",
                    "kids" => "_Player::SetKids",
                    "food" => "_Player::SetFoodStored",
                    "ore" => "_Player::SetOre",
                    _ => "_Player::SetRareOre",
                };
                crate::luaexport::player_calls(&first.calls, api, p.id).next().is_some()
            });
            players.insert(p.id, (units, level));
        }
        let walls = games[0].walls.clone();
        if games.iter().any(|g| g.walls != walls) {
            notes.push(format!("{name}: the tubes and walls change from game to game (the first game's are used)"));
        }
        let (beacons, rolled) = beacons_of(&games, &mut notes, name);
        let after = games[0].after.iter().filter(|(t, n)| games.iter().all(|g| g.after.get(t) == Some(n))).map(|(t, n)| (*t, *n)).collect();
        let units_made = games[0].units.values().map(Vec::len).sum();
        let edits = edits_lua(&games);
        let sequence = games[0].sequence.clone();
        let wrecks = wrecks_lua(&games);
        levels.push(Difficulty { players, walls, beacons, rolled, after, units_made, sequence, edits, wrecks, games: i });
        firsts.push(first);
    }

    // The computer player, when the setup makes AI groups for it (CES1, CPS2): ai.rs writes its setup as `setup_ai`,
    // its units included, from Normal's; the colony's own lists leave its units out. It is the player whose groups
    // the setup makes first (player 3 in Eden 4, with three players), else player 2.
    let ai_player: i32 = firsts[1]
        .calls
        .iter()
        .filter(|c| c.phase == "InitProc" && c.api.starts_with("Create") && c.api.ends_with("Group"))
        .find_map(|c| c.args.first()?.strip_prefix("Player")?.parse().ok())
        .filter(|p| *p > 0)
        .unwrap_or(1);
    let player_arg = format!("Player{ai_player}");
    let makes_groups = firsts[1].calls.iter().any(|c| {
        c.phase == "InitProc" && c.api.starts_with("Create") && c.api.ends_with("Group") && c.args.first() == Some(&player_arg)
    });
    // A campaign mission's computer player may make no groups in the setup and all of them in its trigger functions
    // (Eden 7's waves): it is one all the same, with an empty setup, so those functions convert.
    let campaign_ai = desc.mission_type > 0 && desc.num_players >= 2;
    let mut ai = None;
    if makes_groups || campaign_ai {
        let opts = Options { difficulty: 1, call_log: true, ..Options::from_env() };
        match crate::ai::setup_or_empty(dll, &opts, ai_player, pad, size.0, campaign_ai) {
            Ok(a) => ai = a,
            Err(why) => notes.push(format!("the computer player isn't converted: {why}")),
        }
    }
    // Its units are `setup_ai`'s: out of every difficulty's lists, the camera kept.
    if ai.is_some() {
        for l in &mut levels {
            if let Some((u, _)) = l.players.get_mut(&ai_player) {
                *u = Units { view: u.view, ..Default::default() };
            }
            l.units_made = l.players.values().map(|(u, _)| u.fixed.len() + u.rolled.len() + u.kit.len()).sum();
        }
    }

    // Tubes and walls the setup makes later than the placement would (made first, they stand where the setup's units
    // then can't): those after the computer player's first group (Plymouth 10's around the guard posts), made after
    // `setup_ai` where the setup makes them; those on tiles the setup's map edits changed before them (Plymouth 12's 7
    // tubes), made after the edits. Normal's, when every difficulty makes them.
    let mut late_walls: Vec<(u32, Vec<Wall>)> = Vec::new();
    let mut edit_walls: Vec<((i32, i32), String)> = Vec::new();
    {
        let first_group = ai.as_ref().and_then(|a| a.first_group()).map(|g| g as u32);
        for r in wall_runs(&firsts[1].calls, pad) {
            if r.over_edits {
                edit_walls.extend(r.walls);
            } else if first_group.is_some_and(|g| r.stub >= g) {
                late_walls.push((r.stub, r.walls));
            }
        }
        let moved: Vec<&((i32, i32), String)> = late_walls.iter().flat_map(|(_, w)| w).chain(&edit_walls).collect();
        if levels.iter().all(|l| moved.iter().all(|w| l.walls.contains(*w))) {
            for l in &mut levels {
                l.walls.retain(|w| !moved.contains(&w));
            }
        } else {
            notes.push("the tubes and walls the setup makes late change with the difficulty (made first)".into());
            late_walls.clear();
            edit_walls.clear();
        }
    }

    // Beacons the setup makes after some of its triggers (CES1's six between GiveAIOre's and ExpandBase1_A's): a
    // beacon of a random kind draws as it's made, and a trigger with a time window as it's made, so they're made in
    // `on_init` right after the trigger made before them, in Normal's kinds.
    let mut trigger_beacons: Vec<(u32, String)> = Vec::new();
    {
        let mut last_trigger: Option<u32> = None;
        let mut by_tile: Vec<((i32, i32), u32)> = Vec::new();
        for c in firsts[1].calls.iter().filter(|c| c.phase == "InitProc") {
            if c.api.starts_with("Create") && (c.api.ends_with("Trigger") || c.api.ends_with("Condition")) {
                last_trigger = c.ret.rsplit('#').next().and_then(|n| n.trim().parse().ok()).or(last_trigger);
            } else if c.api == "TethysGame::CreateBeacon"
                && let Some(n) = last_trigger
            {
                let a: Vec<&str> = c.args.iter().flat_map(|a| a.split(',')).map(str::trim).collect();
                if let [_, x, y, ..] = a[..]
                    && let (Some(x), Some(y)) = (int(x), int(y))
                {
                    by_tile.push(((x - pad, y), n));
                }
            }
        }
        // Only beacons on a fixed tile, made so at every difficulty.
        by_tile.retain(|(t, _)| levels.iter().all(|l| l.beacons.iter().any(|b| b.0 == *t)));
        for (t, n) in &by_tile {
            if let Some((_, b)) = levels[1].beacons.iter().find(|b| b.0 == *t) {
                trigger_beacons.push((*n, format!("  game.create_beacon{{ {}, at = {} }}\n", beacon_kind_of(b), at(*t))));
            }
        }
        for l in &mut levels {
            l.beacons.retain(|b| !by_tile.iter().any(|(t, _)| *t == b.0));
        }
    }

    // What every difficulty makes alike.
    let ids: BTreeSet<i32> = levels.iter().flat_map(|l| l.players.keys().copied()).collect();
    let mut common: BTreeMap<i32, Vec<Unit>> = BTreeMap::new();
    for &id in &ids {
        let lists: Vec<&[Unit]> = levels.iter().map(|l| l.players.get(&id).map_or(&[][..], |(u, _)| &u.fixed[..])).collect();
        common.insert(id, common_units(&lists));
    }
    // The placement makes no Garage parking: a vehicle parked alike at every difficulty is made on its tile.
    if common.values().flatten().any(|u| u.garage.is_some()) {
        notes.push("a vehicle the setup parks in a Garage at every difficulty is made outside it (the placement parks none)".into());
    }
    // Units alike at every difficulty that the setup makes after the computer player's base (Eden 4: the human's
    // vehicles last): the placement, made before on_init, would number them first; they're made right after
    // `setup_ai` instead, in the order made.
    let mut late_units: Vec<(i32, Unit)> = Vec::new();
    // By player, how many of its shared units the placement makes (the rest are late).
    let mut placed_of: BTreeMap<i32, usize> = common.iter().map(|(id, us)| (*id, us.len())).collect();
    if ai.is_some() {
        let calls: Vec<&op2_missionhost::runtime::CallRec> = firsts[1].calls.iter().filter(|c| c.phase == "InitProc").collect();
        let owner_of = |c: &op2_missionhost::runtime::CallRec| -> Option<i32> {
            (c.api == "TethysGame::CreateUnit").then(|| c.args.iter().flat_map(|a| a.split(',')).nth(4)?.trim().parse().ok()).flatten()
        };
        let ai_arg = format!("Player{ai_player}");
        let ai_first = calls.iter().position(|c| {
            owner_of(c) == Some(ai_player)
                || (c.api == "TethysGame::CreateUnitBlock" && c.args.first() == Some(&ai_arg))
                || c.api == "ScGroup::AddUnits"
        });
        if let Some(k) = ai_first {
            for (&id, us) in common.iter() {
                if id == ai_player || us.is_empty() {
                    continue;
                }
                let before = calls[..k].iter().filter(|c| owner_of(c) == Some(id)).count();
                if before < us.len() {
                    late_units.extend(us[before..].iter().cloned().map(|u| (id, u)));
                    placed_of.insert(id, before);
                }
            }
        }
    }
    let common_walls: BTreeSet<((i32, i32), String)> =
        levels[0].walls.iter().filter(|w| levels.iter().all(|l| l.walls.contains(w))).cloned().collect();
    let mut common_beacons: Vec<((i32, i32), Beacon)> =
        levels[0].beacons.iter().filter(|b| levels.iter().all(|l| l.beacons.contains(b))).cloned().collect();
    // A difficulty's own beacon made before one alike at every difficulty (CPP1's 181,12, second of its beacons): every
    // beacon stays in the difficulty's list, in the order made, since the game numbers them so (and draws their random
    // kinds in that order).
    if levels.iter().any(|l| {
        let first_common = l.beacons.iter().position(|b| common_beacons.contains(b));
        let last_own = l.beacons.iter().rposition(|b| !common_beacons.contains(b));
        matches!((first_common, last_own), (Some(c), Some(o)) if o > c)
    }) {
        // Except those a mine of the placement stands on: the placement makes them first, or the mine has no beacon
        // (CreateUnit faults: CES1, CPS2). A mine's tile is its beacon's or the one east of it (CPS2's at 225,52).
        let mines = ["CommonOreMine", "RareOreMine", "MagmaWell"];
        let under: Vec<(i32, i32)> = common.values().flatten().filter(|u| mines.contains(&lua_name(u.type_id).as_str())).map(|u| u.tile).collect();
        common_beacons.retain(|b| under.iter().any(|t| (t.0 - b.0.0).abs() <= 1 && (t.1 - b.0.1).abs() <= 1));
    }
    // A difficulty's own units made among those alike at every difficulty (CEP1's Normal adds five between its
    // structures and its trucks): each difficulty makes its units and fixed beacons as one list in the order made
    // (`made`), since the game numbers them so, when every unit stands on a fixed tile.
    let all_fixed = levels.iter().all(|l| l.players.values().all(|(u, _)| u.rolled.is_empty() && u.kit.is_empty()));
    let interleaved = levels.iter().any(|l| {
        l.players.iter().any(|(id, (u, _))| {
            let c = common.get(id).map_or(&[][..], |c| &c[..]);
            u.fixed.len() > c.len() && u.fixed[..c.len()] != *c
        })
    });
    let in_order = all_fixed && interleaved;
    if in_order {
        common.values_mut().for_each(Vec::clear);
        common_beacons.clear();
    }
    let same_rolled = levels.iter().all(|l| l.rolled == levels[0].rolled);

    // Day and night (each difficulty's where they differ, written below), morale, lava: from Normal's first game.
    let normal = &firsts[1];
    let everywhere = last_arg(&normal.calls, "TethysGame::SetDaylightEverywhere");
    let moves = last_arg(&normal.calls, "TethysGame::SetDaylightMoves");
    let light = last_arg(&normal.calls, "GameMap::SetInitialLightLevel");
    let morale = morale_calls(&normal.calls);
    let lava = lava_rects(&normal.calls, pad, size);
    for (i, r) in firsts.iter().enumerate() {
        let n = LEVEL_NAMES[i];
        let set = |api: &str| last_arg(&r.calls, api).is_some();
        if set("TethysGame::SetDaylightEverywhere") != everywhere.is_some() || set("TethysGame::SetDaylightMoves") != moves.is_some() {
            notes.push(format!("{n}: day and night are set differently (Normal's is used)"));
        }
        if lava_rects(&r.calls, pad, size) != lava {
            notes.push(format!("{n}: where lava may flow is set differently (Normal's is used)"));
        }
        let made: usize = levels[i].players.values().map(|(u, _)| u.fixed.len() + u.rolled.len() + u.kit.len()).sum();
        // The computer player's units are `setup_ai`'s when it's converted, so they aren't counted on either side.
        let there = r
            .setup
            .objects
            .iter()
            .filter(|o| matches!(o.kind, crate::setup::Kind::Vehicle | crate::setup::Kind::Structure))
            .filter(|o| ai.is_none() || o.owner != ai_player)
            .count();
        if made != there {
            notes.push(format!("{n}: units are made other ways than CreateUnit ({there} on the map, {made} made)"));
        }
    }
    // The win: victory and failure conditions on triggers that call nothing (victory.rs, as for multiplayer).
    let win = crate::victory::convert(&normal.setup.triggers, None, true, &mut notes);
    // The other difficulties' wins, and the functions their triggers call (Plymouth 2's Hard adds ModerateQuake3Hard).
    let mut other_notes = Vec::new();
    let other_wins: Vec<Option<crate::victory::Win>> =
        (0..3).map(|i| (i != 1).then(|| crate::victory::convert(&firsts[i].setup.triggers, None, true, &mut other_notes))).collect();
    let mut all_callbacks = win.callbacks.clone();
    for (i, w) in other_wins.iter().enumerate() {
        if let Some(w) = w {
            all_callbacks.extend(w.callbacks.iter().cloned());
            all_callbacks.extend(crate::timed::callbacks_of(&firsts[i].setup.triggers, &w.handled));
        }
    }
    let ai_player_of = ai.as_ref().map(|a| a.player);
    // Disasters and other timed events (timed.rs, as for multiplayer): Tutorial 3's earthquake.
    let timed = crate::timed::convert(dll, 0, pad, &normal.setup.triggers, &BTreeSet::new(), &win.handled, &all_callbacks, ai.as_mut(), &mut notes);
    // Each difficulty's own setup of the computer player (Eden 6's Easy base has a Microwave Lynx more), merged as
    // `on_init`'s lines are; after the trigger functions, so their unit blocks keep Normal's numbers.
    if let Some(a) = ai.as_mut() {
        let mut bodies = Vec::new();
        for level in [0, 2] {
            let opts = Options { difficulty: level, call_log: true, ..Options::from_env() };
            match crate::ai::setup_or_empty(dll, &opts, ai_player, pad, size.0, true) {
                Ok(Some(other)) => bodies.push(a.add_level(&other, LEVEL_NAMES[level as usize])),
                Ok(None) => bodies.push(String::new()),
                Err(why) => {
                    notes.push(format!("the computer player's {} setup isn't converted (Normal's is used): {why}", LEVEL_NAMES[level as usize]));
                    bodies.push(a.setup_body().to_string());
                }
            }
        }
        let texts = [bodies[0].clone(), a.setup_body().to_string(), bodies[1].clone()];
        if texts.iter().any(|t| *t != texts[1]) {
            let merged = merge_levels(&texts);
            a.set_setup_body(format!("  local difficulty = DIFFICULTIES[players[1].difficulty] and players[1].difficulty or \"Normal\"\n{merged}"));
        }
    }
    let left: Vec<&crate::triggers::Trigger> =
        normal.setup.triggers.iter().filter(|t| !win.handled.contains(&t.id) && !timed.handled.contains(&t.id)).collect();
    let mut callbacks: Vec<String> = Vec::new();
    for t in &left {
        let what = t.callback.clone().unwrap_or_else(|| t.kind.clone());
        if !callbacks.contains(&what) {
            callbacks.push(what);
        }
    }
    if !left.is_empty() {
        notes.push(format!("{} of the setup's triggers aren't converted yet (they call {})", left.len(), callbacks.join(", ")));
    }
    // AIProc and the trigger functions, as a replay with nobody playing sees them.
    let opts = Options { seed: u64::from(seed(0)), difficulty: 1, call_log: true, ..Options::from_env() };
    let replay = crate::calls::record_with(dll, &opts, REPLAY_MARKS, &[])?;
    let aiproc = replay.iter().filter(|c| c.phase == "AIProc").count();
    if aiproc > 0 {
        notes.push(format!("AIProc isn't converted yet ({aiproc} game calls in the first {REPLAY_MARKS} marks with nobody playing)"));
    }
    // The game makes empty fight groups for the unused player slots after the setup (0x402F20); only groups the
    // setup makes are the mission's.
    let made_groups = normal.calls.iter().any(|c| c.phase == "InitProc" && c.api.starts_with("Create") && c.api.ends_with("Group"));
    if made_groups && ai.is_none() {
        notes.push("the setup's AI groups aren't converted yet".into());
    }

    // The setup's map edits, when every difficulty makes the same.
    let edit_lines: Vec<String> = match levels.iter().map(|l| l.edits.clone()).collect::<Result<Vec<_>, _>>() {
        Ok(all) if all.iter().all(|e| *e == all[0]) => all[0].clone(),
        Ok(_) => {
            notes.push("the setup's map edits change with the difficulty (not converted)".into());
            Vec::new()
        }
        Err(why) => {
            notes.push(format!("{why} (not converted)"));
            Vec::new()
        }
    };

    // ---- writing ----
    let stem = dll.file_stem().map_or("mission".into(), |s| s.to_string_lossy().into_owned());
    let base_name = format!("{stem}Lua");
    let name = format!("{} (Lua)", desc.level_desc.trim());
    let file = dll.file_name().map_or(stem.clone(), |f| f.to_string_lossy().into_owned());
    // A unit's Lua fields, without its tile and player; the placement names a ConVec's cargo `kit` and `kit_topping`.
    let unit_fields = |u: &Unit, placement: bool| -> String {
        let mut s = format!("type = \"{}\"", lua_name(u.type_id));
        if u.weapon > 0 {
            let key = if placement && u.type_id == crate::luaexport::map_id("CON_TRUCK") { "kit" } else { "weapon" };
            let _ = write!(s, ", {key} = \"{}\"", lua_name(u.weapon));
        }
        if u.topping > 0 {
            let key = if placement { "kit_topping" } else { "topping" };
            let _ = write!(s, ", {key} = \"{}\"", lua_name(u.topping));
        }
        if u.vehicle {
            let _ = write!(s, ", direction = \"{}\"", DIRECTIONS[u.rotation as usize]);
            if !u.lights {
                s.push_str(", lights = false");
            }
        }
        if let Some(c) = cargo_name(u.cargo.0) {
            let _ = write!(s, ", cargo = \"{c}\", amount = {}", u.cargo.1);
        }
        // `make_unit` in on_init parks a vehicle in its Garage; the placement can't, so it makes the vehicle outside.
        if let Some((bay, (x, y))) = u.garage.filter(|_| !placement) {
            let _ = write!(s, ", garage = {{ bay = {}, at = {{ {}, {} }} }}", bay + 1, x + 1, y + 1);
        }
        s
    };
    let beacon_kind = beacon_kind_of;
    let rel = |(x, y): (i32, i32)| format!("{{ {x}, {y} }}");
    let first = &firsts[1].setup.players;

    let mut p = String::new();
    let _ = writeln!(p, "-- {base_name}.placement.lua  -  {name}");
    let _ = writeln!(p, "-- Converted by op2dll from {file}: what its setup makes at every difficulty. What changes with the");
    let _ = writeln!(p, "-- difficulty (more units on Easy, colonists, stores, research) and the beacons it rolls are in {base_name}.lua.");
    let _ = writeln!(p, "-- Coordinates are the in-game status-bar ones (map file tile + 1).");
    let _ = writeln!(p, "return {{");
    let _ = writeln!(p, "  name = {}, map = {}, tech = {},", quoted(&name), quoted(&desc.map_name), quoted(&desc.techtree_name));
    let _ = writeln!(p, "  type = \"{}\", max_tech = {}, players_count = {},", crate::opm::mission_type(desc.mission_type), desc.max_tech_level, desc.num_players);
    let _ = writeln!(p, "\n  players = {{");
    for pl in first {
        let colour = COLORS.get(pl.color as usize).copied().unwrap_or("Blue");
        let _ = writeln!(
            p,
            "    [{}] = {{ colony = \"{}\", human = {}, color = \"{colour}\" }},",
            pl.id + 1,
            if pl.eden { "Eden" } else { "Plymouth" },
            pl.human
        );
    }
    let _ = writeln!(p, "  }},\n\n  units = {{");
    for (id, us) in &common {
        if us.is_empty() {
            continue;
        }
        let _ = writeln!(p, "    -- Player {}", id + 1);
        for u in us.iter().take(placed_of[id]) {
            let _ = writeln!(p, "    {{ {}, player = {}, at = {} }},", unit_fields(u, true), id + 1, at(u.tile));
        }
    }
    let _ = writeln!(p, "  }},\n\n  walls = {{");
    for (t, w) in &common_walls {
        let _ = writeln!(p, "    {{ type = \"{}\", at = {} }},", wall_type(w), at(*t));
    }
    let _ = writeln!(p, "  }},\n\n  beacons = {{");
    // A beacon the setup makes after some of its units comes after as many of the placement's, as Normal makes it when
    // the placement has every unit Normal makes (a difficulty with more units, CPP1's Easy, makes them after the
    // placement).
    let placed: usize = placed_of.values().sum();
    let normal_level = &levels[1.min(levels.len() - 1)];
    // A beacon Normal makes after all its units, some of them difficulty's (CPS1 has none alike at every difficulty), is
    // made after the difficulty's units instead (`late_beacons`).
    let late: Vec<((i32, i32), Beacon)> = common_beacons
        .iter()
        .filter(|(t, _)| normal_level.units_made > placed && normal_level.after.get(t) == Some(&normal_level.units_made))
        .cloned()
        .collect();
    for (t, b) in common_beacons.iter().filter(|b| !late.contains(b)) {
        let after = normal_level.after.get(t).copied().filter(|n| normal_level.units_made == placed && *n > 0 && *n <= placed);
        let after = after.map_or(String::new(), |n| format!(", after = {n}"));
        let _ = writeln!(p, "    {{ {}, at = {}{after} }},", beacon_kind(b), at(*t));
    }
    let _ = writeln!(p, "  }},\n}}");

    let mut m = String::new();
    let _ = writeln!(m, "-- {base_name}.lua  -  {name}");
    let _ = writeln!(m, "-- Converted by op2dll from {file}. With {base_name}.placement.lua (what every difficulty makes alike),");
    let _ = writeln!(m, "-- this makes the colony's start: what changes with the difficulty, and the beacons the setup rolls.");
    let _ = writeln!(m, "-- Games run: {}.", (0..3).map(|i| format!("{} {}", LEVEL_NAMES[i], levels[i].games)).collect::<Vec<_>>().join(", "));
    let _ = writeln!(m, "-- Coordinates are the in-game status-bar ones (map file tile + 1); a kit's are relative to its start.\n");
    let write_rolled = |m: &mut String, ind: &str, rolled: &[Rolled]| {
        let _ = writeln!(m, "{ind}rolled = {{");
        for r in rolled {
            match r {
                Rolled::Spots { count, kinds, spots, table } => {
                    let _ = writeln!(m, "{ind}  {{ count = {count}, kinds = {{");
                    for k in kinds {
                        let _ = writeln!(m, "{ind}      {{ {} }},", beacon_kind(k));
                    }
                    let (field, list) = match table {
                        Some(t) => ("table", t),
                        None => ("spots", spots),
                    };
                    let rows: Vec<String> = list.chunks(8).map(|c| c.iter().map(|s| at(*s)).collect::<Vec<_>>().join(", ")).collect();
                    let rows = rows.join(&format!(",\n{ind}      "));
                    let _ = writeln!(m, "{ind}    }},\n{ind}    {field} = {{\n{ind}      {rows},\n{ind}    }} }},");
                }
                Rolled::Area { kind, rect, draws } => {
                    let how = draws.map_or(String::new(), |(n, x, (bx, by))| {
                        format!(", draws = {{ {}, {} }}, x_draw = {}, base = {{ {}, {} }}", n[0], n[1], x + 1, bx + 1, by + 1)
                    });
                    let _ = writeln!(
                        m,
                        "{ind}  {{ {}, area = {{ {}, {}, {}, {} }}{how} }},",
                        beacon_kind(kind),
                        rect[0] + 1,
                        rect[1] + 1,
                        rect[2] + 1,
                        rect[3] + 1
                    );
                }
                Rolled::Kinds { tile, kinds } => {
                    let ways: Vec<String> = kinds.iter().map(|k| format!("{{ {}, at = {} }}", beacon_kind(k), at(*tile))).collect();
                    let _ = writeln!(m, "{ind}  {{ ways = {{ {} }} }},", ways.join(", "));
                }
            }
        }
        let _ = writeln!(m, "{ind}}},");
    };
    if same_rolled {
        let _ = writeln!(m, "-- Beacons rolled each game, alike at every difficulty: `count` of the `spots` (no two alike) with the");
        let _ = writeln!(m, "-- `kinds` by pick, anywhere in an `area` (x1, y1, x2, y2), or a tile's kind picked from `ways`.");
        let _ = writeln!(m, "local BEACONS = {{");
        write_rolled(&mut m, "  ", &levels[0].rolled);
        let _ = writeln!(m, "}}\n");
    }
    let _ = writeln!(m, "-- By difficulty (players[1].difficulty), for each player: colonists, stores, research, Structure Factory");
    let _ = writeln!(m, "-- kits by bay, the camera, units on fixed tiles, units each on a tile rolled in its `area`, and a kit of");
    let _ = writeln!(m, "-- units on a start rolled from `starts` or anywhere in `start_area`; then the tubes, walls and beacons");
    let _ = writeln!(m, "-- only this difficulty makes{}.", if same_rolled { "" } else { ", and the beacons it rolls" });
    let mut blocks: Vec<String> = Vec::new();
    let mut any_late = false;
    // Each difficulty's entry of `DIFFICULTIES`; entries all alike are written once, below.
    for l in &levels {
        let mut block = String::new();
        let m = &mut block;
        let _ = writeln!(m, "    players = {{");
        for (id, (us, lv)) in &l.players {
            let _ = writeln!(m, "      [{}] = {{", id + 1);
            let nums: Vec<String> = lv.numbers.iter().map(|(k, v)| format!("{k} = {v}")).collect();
            if !nums.is_empty() {
                let _ = writeln!(m, "        {},", nums.join(", "));
            }
            let research: Vec<String> = lv.research.iter().map(i32::to_string).collect();
            let _ = writeln!(m, "        research = {{ {} }},", research.join(", "));
            if let Some(tl) = lv.tech_level {
                let _ = writeln!(m, "        tech_level = {tl},");
            }
            if !lv.kits.is_empty() {
                let kits: Vec<String> = lv
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
                let _ = writeln!(m, "        kits = {{ {} }},", kits.join(", "));
            }
            if let Some(v) = us.view {
                let _ = writeln!(m, "        view = {},", if us.starts.is_empty() { at(v) } else { rel(v) });
            }
            // The fixed units placement.lua doesn't make; in order, they're all in `made` instead.
            let extra = if in_order { Vec::new() } else { without(&us.fixed, common.get(id).map_or(&[][..], |c| &c[..])) };
            let _ = writeln!(m, "        units = {{");
            for u in &extra {
                let _ = writeln!(m, "          {{ {}, at = {} }},", unit_fields(u, false), at(u.tile));
            }
            let _ = writeln!(m, "        }},");
            if !us.rolled.is_empty() {
                let _ = writeln!(m, "        rolled = {{");
                for (u, a) in &us.rolled {
                    let _ = writeln!(m, "          {{ {}, area = {{ {}, {}, {}, {} }} }},", unit_fields(u, false), a[0] + 1, a[1] + 1, a[2] + 1, a[3] + 1);
                }
                let _ = writeln!(m, "        }},");
            }
            if !us.starts.is_empty() {
                let s = &us.starts;
                let (x1, x2) = (s.iter().map(|p| p.0).min().unwrap_or(0), s.iter().map(|p| p.0).max().unwrap_or(0));
                let (y1, y2) = (s.iter().map(|p| p.1).min().unwrap_or(0), s.iter().map(|p| p.1).max().unwrap_or(0));
                let area = ((x2 - x1 + 1) * (y2 - y1 + 1)) as usize;
                // As for beacons: starts that fill half their rectangle are an area, else a list.
                if s.len() >= AREA_MIN && s.len() * 2 >= area {
                    let _ = writeln!(m, "        start_area = {{ {}, {}, {}, {} }},  -- {} of its {area} tiles seen", x1 + 1, y1 + 1, x2 + 1, y2 + 1, s.len());
                } else {
                    let rows: Vec<String> = s.chunks(8).map(|c| c.iter().map(|s| at(*s)).collect::<Vec<_>>().join(", ")).collect();
                    let _ = writeln!(m, "        starts = {{  -- {} seen\n          {},\n        }},", s.len(), rows.join(",\n          "));
                }
                let _ = writeln!(m, "        kit = {{");
                for u in &us.kit {
                    let _ = writeln!(m, "          {{ {}, at = {} }},", unit_fields(u, false), rel(u.tile));
                }
                let _ = writeln!(m, "        }},");
            }
            let _ = writeln!(m, "      }},");
        }
        let _ = writeln!(m, "    }},");
        let walls: Vec<String> =
            l.walls.iter().filter(|w| !common_walls.contains(w)).map(|(t, w)| format!("{{ \"{}\", {} }}", wall_type(w), at(*t))).collect();
        let _ = writeln!(m, "    walls = {{");
        for c in walls.chunks(4) {
            let _ = writeln!(m, "      {},", c.join(", "));
        }
        let _ = writeln!(m, "    }},");
        if in_order {
            // Units and beacons in the order the first game made them. `next` is the next rolled group, `left` how many
            // more beacons of the current group's picks still follow (they're made by its one `roll` entry).
            let _ = writeln!(m, "    made = {{");
            let (mut next, mut left) = (0, 0);
            for thing in &l.sequence {
                match thing {
                    Made::Unit(id, i) => {
                        if let Some(u) = l.players.get(id).and_then(|(us, _)| us.fixed.get(*i)) {
                            let _ = writeln!(m, "      {{ {}, player = {}, at = {} }},", unit_fields(u, false), id + 1, at(u.tile));
                        }
                    }
                    Made::Beacon(t) => {
                        if let Some((_, b)) = l.beacons.iter().find(|(bt, _)| bt == t) {
                            let _ = writeln!(m, "      {{ {}, at = {}, beacon = true }},", beacon_kind(b), at(*t));
                        } else if left == 0 {
                            // A rolled beacon: the next rolled group starts here.
                            if let Some(r) = l.rolled.get(next) {
                                let _ = writeln!(m, "      {{ roll = {} }},", next + 1);
                                left = match r {
                                    Rolled::Spots { count, .. } => *count,
                                    _ => 1,
                                } - 1;
                                next += 1;
                            }
                        } else {
                            left -= 1;
                        }
                    }
                }
            }
            let _ = writeln!(m, "    }},");
        }
        // This difficulty's beacons made after all its units, some of them its own, come after them too.
        let own_late = |b: &((i32, i32), Beacon)| !in_order && l.units_made > placed && l.after.get(&b.0) == Some(&l.units_made);
        let (late_here, early): (Vec<_>, Vec<_>) = l
            .beacons
            .iter()
            .filter(|b| !in_order && (!common_beacons.contains(b) || late.contains(b)))
            .partition(|b| late.contains(b) || own_late(b));
        let _ = writeln!(m, "    beacons = {{");
        for (t, b) in early {
            let _ = writeln!(m, "      {{ {}, at = {} }},", beacon_kind(b), at(*t));
        }
        let _ = writeln!(m, "    }},");
        if !late_here.is_empty() {
            any_late = true;
            let _ = writeln!(m, "    late_beacons = {{");
            for (t, b) in late_here {
                let _ = writeln!(m, "      {{ {}, at = {} }},", beacon_kind(b), at(*t));
            }
            let _ = writeln!(m, "    }},");
        }
        if !same_rolled {
            write_rolled(m, "    ", &l.rolled);
        }
        blocks.push(block);
    }
    // A setup that ignores the difficulty (the tutorials) gets one entry, which every difficulty falls back to.
    let alike = blocks.iter().all(|b| *b == blocks[0]);
    if alike {
        let _ = writeln!(m, "-- The setup makes the same at every difficulty: `Normal` is used for all three.");
    }
    let _ = writeln!(m, "local DIFFICULTIES = {{");
    for (i, b) in blocks.iter().enumerate().filter(|(i, _)| !alike || *i == 1) {
        let _ = writeln!(m, "  {} = {{\n{b}  }},", LEVEL_NAMES[i]);
    }
    let _ = writeln!(m, "}}\n");
    if !lava.is_empty() {
        let _ = writeln!(m, "-- Where lava may flow (x1, y1, x2, y2).");
        let _ = writeln!(m, "local LAVA_POSSIBLE = {{");
        for r in &lava {
            let _ = writeln!(m, "  {{ {}, {}, {}, {} }},", r[0], r[1], r[2], r[3]);
        }
        let _ = writeln!(m, "}}\n");
    }
    m.push_str(RUNTIME);
    if let Some(a) = &ai {
        m.push('\n');
        m.push_str(&a.header());
    }
    if !timed.functions.is_empty() {
        let _ = writeln!(m, "{}", timed.functions.trim_end());
        let _ = writeln!(m);
    }
    let _ = writeln!(m, "function on_init()");
    let _ = writeln!(m, "  local difficulty = DIFFICULTIES[players[1].difficulty] and players[1].difficulty or \"Normal\"");
    let _ = writeln!(m, "  local d = DIFFICULTIES[difficulty]");
    let _ = writeln!(m, "  print(\"Difficulty: \" .. difficulty)");
    let _ = writeln!(m, "  make_things(d)");
    let rolled_list = if same_rolled { "BEACONS.rolled" } else { "d.rolled" };
    if in_order {
        let _ = writeln!(m, "  make_in_order(d, {rolled_list})");
    } else {
        let _ = writeln!(m, "  roll_beacons({rolled_list})");
    }
    let _ = writeln!(m, "  for id, p in pairs(d.players) do make_player(players[id], p) end");
    if any_late {
        let _ = writeln!(m, "  -- The beacons the original makes after all its units, so that the units keep its ids.");
        let _ = writeln!(m, "  for _, b in ipairs(d.late_beacons or {{}}) do game.create_beacon(b) end");
    }
    if !edit_lines.is_empty() {
        let _ = writeln!(m, "  -- The setup's own map edits, after its units (and the draws it makes for them).");
        if edit_lines.iter().any(|l| l.starts_with("r = ")) {
            let _ = writeln!(m, "  local r");
        }
        for l in &edit_lines {
            let _ = writeln!(m, "  {l}");
        }
    }
    if !edit_walls.is_empty() {
        let _ = writeln!(m, "  -- The tubes and walls the setup makes on tiles its map edits changed, after them.");
        let _ = write!(m, "{}", walls_lua(&edit_walls));
    }
    // Each difficulty's day and night (Easy's daylight everywhere in Eden 11 and Plymouth 10, Hard's night in Eden 9).
    for (api, lua, normal_v) in [("TethysGame::SetDaylightEverywhere", "set_daylight_everywhere", everywhere), ("TethysGame::SetDaylightMoves", "set_daylight_moves", moves)] {
        let Some(v) = normal_v else { continue };
        let on: Vec<bool> = firsts.iter().map(|r| last_arg(&r.calls, api).map_or(v != 0, |x| x != 0)).collect();
        if on.iter().all(|o| *o == on[1]) {
            let _ = writeln!(m, "  game.{lua}({})", on[1]);
        } else {
            let levels: Vec<String> = (0..3).filter(|i| on[*i]).map(|i| format!("difficulty == \"{}\"", LEVEL_NAMES[i])).collect();
            let _ = writeln!(m, "  game.{lua}({})", if levels.is_empty() { "false".to_string() } else { levels.join(" or ") });
        }
    }
    if let Some(v) = light {
        let _ = writeln!(m, "  game.set_initial_light_level({v})");
    }
    let morales: Vec<Vec<String>> = firsts.iter().map(|r| morale_calls(&r.calls)).collect();
    if morales.iter().all(|x| *x == morale) {
        for c in &morale {
            let _ = writeln!(m, "  {c}");
        }
    } else {
        for (i, calls) in morales.iter().enumerate().filter(|(_, c)| !c.is_empty()) {
            let _ = writeln!(m, "  if difficulty == \"{}\" then {} end", LEVEL_NAMES[i], calls.join("; "));
        }
    }
    if !lava.is_empty() {
        let _ = writeln!(m, "  for _, r in ipairs(LAVA_POSSIBLE) do game.set_lava_possible(r) end");
    }
    // The triggers, the win's (as one block, at its first), the computer player's base (at its first group) and the
    // beacons made among them, in the order the setup makes them: the game numbers triggers and groups as they're
    // made (and gives a destroyed one's number to the next), and draws a windowed trigger's time as it's made.
    // Each entry is (the trigger or group it follows, its order among things after that one, its Lua): 0 a trigger
    // or the base, 1 a beacon, 2 wreckage, 3 a marker, 4 a default group's area, 5 late tubes and walls.
    // By difficulty: its win and timed triggers (`own`), numbered as it makes them.
    let mut own: Vec<Vec<(u32, u8, String)>> = Vec::new();
    for (i, r) in firsts.iter().enumerate() {
        let mut list: Vec<(u32, u8, String)> = Vec::new();
        let (w, t) = match &other_wins[i] {
            Some(w) => (w, crate::timed::lines_for(&timed, &r.setup.triggers, &w.handled, ai_player_of, &mut other_notes)),
            None => (&win, crate::timed::lines_for(&timed, &[], &BTreeSet::new(), None, &mut other_notes)),
        };
        let t = if i == 1 { &timed } else { &t };
        if !w.lua.is_empty() {
            list.push((w.handled.iter().min().copied().unwrap_or(0), 0, w.lua.clone()));
        }
        if t.init_by_id.is_empty() {
            list.push((0, 0, t.init.clone()));
        }
        list.extend(t.init_by_id.iter().map(|(id, l)| (*id, 0, l.clone())));
        own.push(list);
    }
    // What every difficulty makes alike, after the trigger or group Normal makes before it.
    let mut entries: Vec<(u32, u8, String)> = Vec::new();
    if let Some(a) = &ai {
        let mut line = format!("\n  -- The computer player's base, where the setup makes it.\n  setup_ai(players[{}])\n", ai_player + 1);
        if !late_units.is_empty() {
            line.push_str("  -- The units the setup makes after it, alike at every difficulty.\n");
            for (id, u) in &late_units {
                let _ = writeln!(line, "  make_unit(players[{}], {{ {} }}, {}, nil, true)", id + 1, unit_fields(u, true), at(u.tile));
            }
        }
        entries.push((a.first_group().map_or(u32::MAX, |g| g as u32), 0, line));
    }
    entries.extend(trigger_beacons.iter().map(|(n, l)| (*n, 1, l.clone())));
    for (n, w) in &late_walls {
        entries.push((*n, 5, format!("  -- The tubes and walls the setup makes here, after the computer player's base.
{}", walls_lua(w))));
    }
    // Wreckage the setup leaves (Eden 9's four, after the computer's groups): made after the trigger or group made
    // before the first, as the setup makes it (a wreck is an object, numbered as it's made), by difficulty when the
    // difficulties differ.
    {
        let mut last: u32 = 0;
        for c in firsts[1].calls.iter().filter(|c| c.phase == "InitProc") {
            if c.api.starts_with("Create") && (c.api.ends_with("Trigger") || c.api.ends_with("Condition") || c.api.ends_with("Group")) {
                last = c.ret.rsplit('#').next().and_then(|n| n.trim().parse().ok()).unwrap_or(last);
            } else if c.api == "TethysGame::CreateWreck" {
                break;
            }
        }
        let lua: Vec<Result<String, String>> = levels.iter().map(|l| l.wrecks.clone()).collect();
        match lua.iter().find_map(|r| r.as_ref().err()) {
            Some(why) => notes.push(format!("the setup's wreckage isn't converted: {why}")),
            None => {
                let lua: Vec<String> = lua.into_iter().map(Result::unwrap_or_default).collect();
                if lua.iter().any(|l| !l.is_empty()) {
                    let mut block = String::new();
                    if lua.windows(2).all(|w| w[0] == w[1]) {
                        block.push_str(&lua[1]);
                    } else {
                        for (i, l) in lua.iter().enumerate() {
                            let _ = writeln!(block, "  {} players[1].difficulty == \"{}\" then", if i == 0 { "if" } else { "elseif" }, LEVEL_NAMES[i]);
                            block.extend(l.lines().map(|x| format!("  {x}\n")));
                        }
                        let _ = writeln!(block, "  end");
                    }
                    entries.push((last, 2, block));
                }
            }
        }
    }
    // The setup's markers (a convoy's goal in Eden 6, 9 and 11, Plymouth 6, 9 and 10): each an object numbered as it's
    // made, so made after the trigger or group made before it.
    {
        let markers_of = |r: &Run| -> Vec<(u32, String)> {
            let mut last: u32 = 0;
            let mut out = Vec::new();
            for c in r.calls.iter().filter(|c| c.phase == "InitProc") {
                if c.api.starts_with("Create") && (c.api.ends_with("Trigger") || c.api.ends_with("Condition") || c.api.ends_with("Group")) {
                    last = c.ret.rsplit('#').next().and_then(|n| n.trim().parse().ok()).unwrap_or(last);
                } else if c.api == "TethysGame::PlaceMarker" {
                    // "Unit#0, x, y, type" (0 a circle, 1 DNA, 2 a beaker)
                    let a: Vec<&str> = c.args.iter().flat_map(|a| a.split(',')).map(str::trim).collect();
                    if let [_, x, y, kind, ..] = a[..]
                        && let (Some(x), Some(y), Some(kind)) = (int(x), int(y), int(kind))
                    {
                        let kind = match kind {
                            1 => ", \"DNA\"",
                            2 => ", \"Beaker\"",
                            _ => "",
                        };
                        out.push((last, format!("  game.create_marker({}{kind})\n", at((x - pad, y)))));
                    }
                }
            }
            out
        };
        let all: Vec<Vec<(u32, String)>> = firsts.iter().map(markers_of).collect();
        if all.iter().any(|m| *m != all[1]) {
            notes.push("the setup's markers change with the difficulty (Normal's are made)".into());
        }
        entries.extend(all[1].iter().map(|(n, l)| (*n, 3, l.clone())));
    }
    // The area the setup gives a player's default group (Eden 7, 8, 10 and 12: GetDefaultGroup, then
    // FightGroup::SetRect): the game's own group, which a unit a Spider reprograms joins (0x4031A0).
    {
        let mut last: u32 = 0;
        let mut owner: BTreeMap<String, i32> = BTreeMap::new();
        for c in normal.calls.iter().filter(|c| c.phase == "InitProc") {
            if c.api.starts_with("Create") && (c.api.ends_with("Trigger") || c.api.ends_with("Condition") || c.api.ends_with("Group")) {
                last = c.ret.rsplit('#').next().and_then(|n| n.trim().parse().ok()).unwrap_or(last);
            } else if c.api == "_Player::GetDefaultGroup"
                && let Some(p) = c.args.first().and_then(|a| a.strip_prefix("this=Player")).and_then(|p| p.trim().parse().ok())
            {
                owner.insert(c.ret.rsplit('#').next().unwrap_or("").trim().to_string(), p);
            } else if c.api == "FightGroup::SetRect" {
                let args = c.args.join(",");
                let id = args.split(',').next().and_then(|a| a.rsplit('#').next()).unwrap_or("").trim().to_string();
                let r: Vec<i32> = args.split_once('[').map_or("", |(_, r)| r).split(|ch: char| !ch.is_ascii_digit() && ch != '-').filter_map(int).collect();
                if let (Some(p), &[x1, y1, x2, y2]) = (owner.get(&id), &r[..]) {
                    let (a, b) = (at((x1 - pad, y1)), at((x2 - pad, y2)));
                    let rect = format!("{{ {}, {} }}", a.trim_matches(['{', '}', ' ']), b.trim_matches(['{', '}', ' ']));
                    entries.push((last, 4, format!("  players[{}]:default_group():set_rect({rect})\n", p + 1)));
                }
            }
        }
    }
    // Each difficulty's list in the order it makes them, its triggers named as Normal's alike, merged into one with
    // the difficulties' differences as `if`s.
    let slots = slot_pairs(dll, &firsts).unwrap_or_else(|why| {
        notes.push(format!("the setup's trigger slots weren't read: {why}"));
        vec![(BTreeMap::new(), BTreeSet::new()); 3]
    });
    let mut texts: Vec<String> = Vec::new();
    for (i, list) in own.into_iter().enumerate() {
        let names = trigger_names(&firsts[i].setup.triggers, &normal.setup.triggers, &slots[i], LEVEL_NAMES[i]);
        let mut list = list;
        for (k, sub, l) in &entries {
            let k = if i == 1 { *k } else { level_key(*k, &names) };
            list.push((k, *sub, l.clone()));
        }
        list.sort_by_key(|(id, sub, _)| (*id, *sub));
        let text: String = list.into_iter().map(|(_, _, l)| l).collect();
        texts.push(if i == 1 { text } else { rename_triggers(&text, &names.1) });
    }
    m.push_str(&merge_levels(&texts));
    let _ = writeln!(m, "end");
    if !notes.is_empty() {
        let _ = writeln!(m, "\n-- Not converted:");
        for n in &notes {
            let _ = writeln!(m, "-- - {n}");
        }
    }
    Ok(Lua { base: base_name, name, placement: p, mission: m, notes, ai_players: 0 })
}

/// The Lua helpers every converted colony mission carries: making a unit (cargo, kit, Garage, factory bays), a
/// player's start, tubes and walls, and the beacons in order or rolled as the setup rolls them.
const RUNTIME: &str = r#"local kits_set = false
local function make_unit(player, u, at, kits, exact)
  local made = game.create_unit{ type = u.type, player = player, at = at, direction = u.direction,
                                 weapon = u.weapon, lights = u.lights, exact = exact }
  if u.cargo and made[1] then made[1]:set_cargo(u.cargo, u.amount) end
  -- A ConVec's kit with its weapon (a Guard Post kit's).
  if u.topping and made[1] then made[1]:set_kit(u.weapon, u.topping) end
  -- A vehicle the setup parks in a Garage: its bay, by the Garage's dock.
  if u.garage and made[1] then made[1]:put_in_garage(u.garage.bay, u.garage.at) end
  if u.type == "StructureFactory" and kits and made[1] then
    for bay, kit in pairs(kits) do made[1]:set_factory_cargo(bay, kit[1], kit[2]) end
    kits_set = true
  end
end

-- One player's start at the chosen difficulty.
function make_player(player, p)
  if p.tech_level then player:set_tech_level(p.tech_level) end
  for _, tech in ipairs(p.research) do player:mark_research(tech) end
  -- Only the numbers the setup sets.
  for _, k in ipairs({ "ore", "rare_ore", "food", "workers", "scientists", "kids" }) do
    if p[k] then player[k] = p[k] end
  end
  for _, u in ipairs(p.units) do make_unit(player, u, u.at, p.kits, true) end
  -- As the setup draws them: the game's own MAP_RECT::RandPt.
  for _, u in ipairs(p.rolled or {}) do make_unit(player, u, game.rand_point(u.area), p.kits, true) end
  local start = p.starts and p.starts[game.rand(#p.starts) + 1]
  local s = p.start_area
  -- A start drawn in a row or a column draws only along it (CPS2: one GetRand(91) for x).
  local function along(lo, hi) return hi > lo and lo + game.rand(hi - lo + 1) or lo end
  if s then start = { along(s[1], s[3]), along(s[2], s[4]) } end
  local function place(t) return start and { start[1] + t[1], start[2] + t[2] } or t end
  -- The original asks for every unit of a kit on one tile; the game finds each a place nearby.
  for _, u in ipairs(p.kit or {}) do make_unit(player, u, place(u.at), p.kits, false) end
  -- A Structure Factory alike at every difficulty is the placement's; its bays are the difficulty's (Eden 1).
  if p.kits and not kits_set then
    local f = player:buildings("StructureFactory")[1]
    if f then
      for bay, kit in pairs(p.kits) do f:set_factory_cargo(bay, kit[1], kit[2]) end
    end
  end
  kits_set = false
  if p.view then player:center_view(place(p.view)) end
end

function make_walls(walls)
  for _, w in ipairs(walls) do
    if w[1] == "Tube" then game.create_tube(w[2]) else game.create_wall(w[1], w[2]) end
  end
end

function make_things(d)
  make_walls(d.walls)
  for _, b in ipairs(d.beacons) do game.create_beacon(b) end
end

local function copy(t)
  local c = {}
  for k, v in pairs(t) do c[k] = v end
  return c
end

-- The difficulty's units and beacons in the order the original makes them; `roll = k` rolls the k-th of `rolled`.
function make_in_order(d, rolled)
  for _, t in ipairs(d.made or {}) do
    if t.roll then
      roll_beacons({ rolled[t.roll] })
    elseif t.beacon then
      local b = copy(t)
      b.beacon = nil
      game.create_beacon(b)
    else
      make_unit(players[t.player], t, t.at, d.players[t.player] and d.players[t.player].kits, true)
    end
  end
end

function roll_beacons(rolled)
  for _, r in ipairs(rolled) do
    if r.table then
      -- As the setup draws them: an index into its table, again while it repeats one; all picks, then the beacons.
      local picks, seen = {}, {}
      while #picks < r.count do
        local v = game.rand(#r.table)
        if not seen[v] then seen[v] = true picks[#picks + 1] = v end
      end
      for i, v in ipairs(picks) do
        local b = copy(r.kinds[i])
        b.at = r.table[v + 1]
        game.create_beacon(b)
      end
    elseif r.spots then
      local spots = {}
      for i, s in ipairs(r.spots) do spots[i] = s end
      for i = 1, r.count do
        local b = copy(r.kinds[i])
        b.at = table.remove(spots, game.rand(#spots) + 1)
        game.create_beacon(b)
      end
    elseif r.area then
      local a, b = r.area, copy(r)
      b.area, b.draws, b.x_draw, b.base = nil, nil, nil, nil
      if r.draws then
        -- As the setup draws it: two numbers, one added to the base's x and the other to its y.
        local d = { game.rand(r.draws[1]), game.rand(r.draws[2]) }
        b.at = { r.base[1] + d[r.x_draw], r.base[2] + d[3 - r.x_draw] }
      else
        b.at = { a[1] + game.rand(a[3] - a[1] + 1), a[2] + game.rand(a[4] - a[2] + 1) }
      end
      game.create_beacon(b)
    else
      game.create_beacon(r.ways[game.rand(#r.ways) + 1])
    end
  end
end

"#;

/// Each difficulty's triggers paired with Normal's by where the setup keeps them: a dword of the mission's writable
/// data that holds a trigger's number after the setup at every difficulty, not the same at all three (the handles its
/// functions read, Plymouth 2's CheckForFail's at 0x11008770: #27 on Normal, #28 on Hard). By difficulty, its
/// trigger's number to Normal's (one paired two ways is left out), and the numbers alike in a slot at all three.
fn slot_pairs(dll: &Path, firsts: &[Run]) -> Result<Slots> {
    use op2_x86::pe::{PeFile, Section};
    let data = std::fs::read(dll)?;
    let pe = PeFile::parse(&data).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let sections: Vec<(u32, usize)> = pe
        .sections
        .iter()
        .filter(|s| s.characteristics & Section::WRITE != 0 && s.characteristics & Section::EXECUTE == 0)
        .map(|s| (pe.image_base + s.rva, (s.virtual_size.max(s.raw_size) / 4) as usize))
        .collect();
    // Every writable dword after the setup, by difficulty.
    let mut dwords: Vec<Vec<u32>> = Vec::new();
    for d in 0..3 {
        let opts = Options { seed: u64::from(seed(0)), difficulty: d, ..Options::from_env() };
        let mut all = Vec::new();
        for (at, n) in &sections {
            all.extend(crate::calls::peek(dll, &opts, *at, *n)?);
        }
        dwords.push(all);
    }
    let ids: Vec<BTreeSet<u32>> = firsts.iter().map(|r| r.setup.triggers.iter().map(|t| t.id).collect()).collect();
    let mut out = Vec::new();
    // The numbers alike in a slot at every difficulty (Eden 2's kits, #21 to #25 at all three).
    let same: BTreeSet<u32> = (0..dwords[1].len())
        .filter(|k| (0..3).all(|l| dwords[l].get(*k) == dwords[1].get(*k)) && ids.iter().all(|i| i.contains(&dwords[1][*k])))
        .map(|k| dwords[1][k])
        .collect();
    for d in 0..3 {
        let mut pairs: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
        for k in 0..dwords[1].len() {
            // Only a dword that changes with the difficulty tells: one alike in all three may be any number.
            let alike = (0..3).all(|l| dwords[l].get(k) == dwords[1].get(k));
            if !alike && (0..3).all(|l| dwords[l].get(k).is_some_and(|v| ids[l].contains(v))) {
                pairs.entry(dwords[d][k]).or_default().insert(dwords[1][k]);
            }
        }
        let mut level: BTreeMap<u32, u32> =
            pairs.into_iter().filter(|(_, n)| n.len() == 1).filter_map(|(l, n)| n.first().map(|n| (l, *n))).collect();
        // A Normal trigger two of this difficulty's pair with is left out too.
        let mut seen: BTreeMap<u32, usize> = BTreeMap::new();
        for n in level.values() {
            *seen.entry(*n).or_default() += 1;
        }
        level.retain(|_, n| seen[n] == 1);
        out.push((level, same.clone()));
    }
    Ok(out)
}

/// By difficulty, `slot_pairs`: its triggers paired with Normal's by slots that change with the difficulty, and the
/// numbers in slots alike at all three (each its own in every difficulty, where nothing else pairs it).
type Slots = Vec<(BTreeMap<u32, u32>, BTreeSet<u32>)>;

/// A difficulty's triggers paired with Normal's: by where the setup keeps them (`slots`), then by what they are
/// exactly, then by a number alike in a slot at all three, then by kind, callback and win, each in the order made.
/// Gives the pairs (Normal's number, this difficulty's) by Normal's, and the Lua name of each of this difficulty's:
/// Normal's where paired, its own with the difficulty's initial (`t23h`) where Normal makes none like it.
fn trigger_names(
    level: &[crate::triggers::Trigger],
    normal: &[crate::triggers::Trigger],
    slots: &(BTreeMap<u32, u32>, BTreeSet<u32>),
    name: &str,
) -> TriggerNames {
    // What a trigger is, without the numbers of the stubs it names; and looser, its kind, callback and whether a win.
    let exact = |t: &crate::triggers::Trigger| {
        let what: Vec<&str> = t.what.split('#').map(|p| p.trim_start_matches(|c: char| c.is_ascii_digit())).collect();
        format!("{}|{}|{}|{}|{:?}|{:?}", t.kind, what.join("#"), t.enabled, t.one_shot, t.callback, t.objective.as_ref().map(|o| (o.victory, &o.text)))
    };
    let loose = |t: &crate::triggers::Trigger| format!("{}|{:?}|{:?}", t.kind, t.callback, t.objective.as_ref().map(|o| o.victory));
    let mut pairs: BTreeMap<u32, u32> = slots.0.clone();
    let mut taken: BTreeSet<u32> = pairs.values().copied().collect();
    for (pass, key) in [&exact as &dyn Fn(&crate::triggers::Trigger) -> String, &loose].into_iter().enumerate() {
        // Between the passes, a number alike in a slot at every difficulty.
        if pass == 1 {
            for t in level {
                if !pairs.contains_key(&t.id) && slots.1.contains(&t.id) && !taken.contains(&t.id) && normal.iter().any(|n| n.id == t.id) {
                    pairs.insert(t.id, t.id);
                    taken.insert(t.id);
                }
            }
        }
        let mut free: BTreeMap<String, std::collections::VecDeque<u32>> = BTreeMap::new();
        for t in normal.iter().filter(|t| !taken.contains(&t.id)) {
            free.entry(key(t)).or_default().push_back(t.id);
        }
        for t in level {
            if pairs.contains_key(&t.id) {
                continue;
            }
            if let Some(n) = free.get_mut(&key(t)).and_then(|q| q.pop_front()) {
                pairs.insert(t.id, n);
                taken.insert(n);
            }
        }
    }
    let initial = name.chars().next().map_or('x', |c| c.to_ascii_lowercase());
    let names = level.iter().map(|t| (t.id, pairs.get(&t.id).map_or(format!("t{}{initial}", t.id), |n| format!("t{n}")))).collect();
    let mut by_normal: Vec<(u32, u32)> = pairs.into_iter().map(|(l, n)| (n, l)).collect();
    by_normal.sort_unstable();
    (by_normal, names)
}

/// Where a thing Normal makes after its trigger or group `k` goes in another difficulty's list: after that
/// difficulty's trigger paired with the last of Normal's at or before `k`.
fn level_key(k: u32, names: &TriggerNames) -> u32 {
    names.0.iter().rev().find(|(n, _)| *n <= k).map_or(k, |(_, l)| *l)
}

/// `text` with each trigger variable (`t<number>`, outside strings) renamed by `names`, by number.
fn rename_triggers(text: &str, names: &BTreeMap<u32, String>) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut in_str) = (0, false);
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    while i < b.len() {
        let c = b[i];
        if c == b'"' {
            in_str = !in_str;
        }
        if !in_str && c == b't' && (i == 0 || !ident(b[i - 1])) {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1
                && (j == b.len() || !ident(b[j]))
                && let Some(n) = text[i + 1..j].parse::<u32>().ok().and_then(|n| names.get(&n))
            {
                out.push_str(n);
                i = j;
                continue;
            }
        }
        let ch = text[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Lua lines grouped into whole statements (a table or block over several lines stays one).
fn statements(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut cur, mut depth) = (String::new(), 0i32);
    for line in text.lines() {
        cur.push_str(line);
        cur.push('\n');
        // The line without its strings and comment.
        let mut code = String::new();
        let mut in_str = false;
        let mut prev = ' ';
        for ch in line.chars() {
            if ch == '"' {
                in_str = !in_str;
            } else if !in_str && ch == '-' && prev == '-' {
                code.pop();
                break;
            } else if !in_str {
                code.push(ch);
            }
            prev = ch;
        }
        for ch in code.chars() {
            match ch {
                '(' | '{' | '[' => depth += 1,
                ')' | '}' | ']' => depth -= 1,
                _ => {}
            }
        }
        // Blocks: `elseif` closes one `then` and its own opens the next.
        for w in code.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
            match w {
                "then" | "do" | "function" => depth += 1,
                "end" | "elseif" => depth -= 1,
                _ => {}
            }
        }
        if depth <= 0 {
            out.push(std::mem::take(&mut cur));
            depth = 0;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The longest common subsequence of `a` and `b`: the pairs of indices matched, in order.
fn lcs(a: &[String], b: &[String]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut t = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if a[i] == b[j] { t[i + 1][j + 1] + 1 } else { t[i + 1][j].max(t[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if t[i + 1][j] >= t[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// The three difficulties' `on_init` lines (Easy, Normal, Hard) as one: what all three make alike as it is, and where
/// they differ an `if` on the difficulty, the variables made there declared before it.
fn merge_levels(texts: &[String]) -> String {
    if texts.iter().all(|t| *t == texts[1]) {
        return texts[1].clone();
    }
    // Anchors: Normal's statements Easy and Hard both share (each an LCS against Normal's); between anchors, the gaps.
    let s: Vec<Vec<String>> = texts.iter().map(|t| statements(t)).collect();
    let to_e: BTreeMap<usize, usize> = lcs(&s[1], &s[0]).into_iter().collect();
    let to_h: BTreeMap<usize, usize> = lcs(&s[1], &s[2]).into_iter().collect();
    let mut anchors: Vec<[usize; 3]> = to_e.iter().filter_map(|(n, e)| to_h.get(n).map(|h| [*e, *n, *h])).collect();
    anchors.push([s[0].len(), s[1].len(), s[2].len()]);
    let mut out = String::new();
    let mut declared = BTreeSet::new();
    let mut from = [0usize; 3];
    for a in anchors {
        let gaps: Vec<&[String]> = (0..3).map(|i| &s[i][from[i]..a[i]]).collect();
        out.push_str(&gap_lua(&gaps, &mut declared));
        if a[1] < s[1].len() {
            if let Some(name) = local_name(&s[1][a[1]]) {
                declared.insert(name.to_string());
            }
            out.push_str(&s[1][a[1]]);
        }
        from = [a[0] + 1, a[1] + 1, a[2] + 1];
    }
    out
}

/// The variable a `local name = ...` statement declares.
fn local_name(st: &str) -> Option<&str> {
    let (name, _) = st.trim_start().strip_prefix("local ")?.split_once(" = ")?;
    name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_').then_some(name)
}

/// The statements only some difficulties make here, as an `if` on the difficulty; the variables made there are declared
/// before it unless `declared` (the names declared earlier in `on_init`) already has them.
fn gap_lua(gaps: &[&[String]], declared: &mut BTreeSet<String>) -> String {
    let mut groups: Vec<(Vec<usize>, &[String])> = Vec::new();
    for (i, g) in gaps.iter().enumerate() {
        match groups.iter_mut().find(|(_, x)| *x == *g) {
            Some((ls, _)) => ls.push(i),
            None => groups.push((vec![i], g)),
        }
    }
    groups.retain(|(_, g)| !g.is_empty());
    match &groups[..] {
        [] => return String::new(),
        [(ls, g)] if ls.len() == 3 => return g.concat(),
        _ => {}
    }
    let mut locals: Vec<String> = Vec::new();
    let mut body = String::new();
    for (k, (ls, g)) in groups.iter().enumerate() {
        let cond: Vec<String> = ls.iter().map(|i| format!("difficulty == \"{}\"", LEVEL_NAMES[*i])).collect();
        let _ = writeln!(body, "  {} {} then", if k == 0 { "if" } else { "elseif" }, cond.join(" or "));
        for st in g.iter() {
            let mut st = st.clone();
            if let Some(name) = local_name(&st).map(str::to_string) {
                if declared.insert(name.clone()) {
                    locals.push(name);
                }
                st = st.replacen("local ", "", 1);
            }
            for line in st.lines() {
                if line.is_empty() {
                    body.push('\n');
                } else {
                    let _ = writeln!(body, "  {line}");
                }
            }
        }
    }
    let _ = writeln!(body, "  end");
    let mut out = String::new();
    if !locals.is_empty() {
        let _ = writeln!(out, "  local {}", locals.join(", "));
    }
    out.push_str(&body);
    out
}

/// A difficulty's triggers by Normal's (`trigger_names`): the pairs by Normal's number, and each one's Lua name.
type TriggerNames = (Vec<(u32, u32)>, BTreeMap<u32, String>);
