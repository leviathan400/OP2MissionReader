//! A multiplayer mission's setup as a replay of the original's: `on_init` makes the same `GetRand` draws in the same
//! order, and from them and the lobby the same calls, units, beacons and walls in the same order, so a game of the Lua
//! mission is the original's game (the same object numbers and random numbers), not one like it.
//!
//! The setup runs over many seeds and lobbies (player count, each player's colony and resource level, the initial
//! vehicles, day and night, morale, disasters). Each run is a list of tokens: draws (consecutive draws of one range
//! taken together) and calls; the lobby's answers (`NoPlayers`, `IsEden`, `Difficulty`, ...) are inputs the Lua reads
//! from the game. The runs are aligned on what they share (LCS on the tokens' shapes); where they differ, the Lua
//! branches on the earlier draw or lobby input that decides it. Each argument of a call is a constant, or follows an
//! earlier draw or input: by arithmetic (`x = 10 + r`) or a table. A run of draws of one range that stops when it has
//! drawn k different values is the setups' way of picking k things (starts, bases), and becomes `pick(range, k)`; one
//! draw redrawn while it equals an earlier draw (or an earlier draw plus a fixed offset) becomes `draw_avoid`. A block
//! made once per initial vehicle becomes a `for` loop.
//!
//! The replay ends at the setup's first trigger or AI group: the win and the disasters' triggers (victory.rs, timed.rs)
//! follow it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use anyhow::Result;
use op2_missionhost::runtime::{CallRec, NetSlot, NetStart, Options};

use crate::luaexport::{DIRECTIONS, cargo_name, lua_name, map_id, quoted, variant_name};

/// One game's lobby: the answers to what a setup asks the game, as numbers (flags 0 or 1). The Lua reads them by
/// `key_lua`.
#[derive(Clone, Debug)]
struct Env {
    /// Players.
    n: i64,
    /// Each player's colony (1: Eden).
    eden: Vec<i64>,
    /// Each player's resource level (0 to 2).
    level: Vec<i64>,
    /// Initial vehicles (0, 4, 8 or 12).
    units: i64,
    /// `UsesDayNight`'s answer (1: day and night on).
    day_night: i64,
    /// `UsesMorale`'s answer: 1 when the lobby's morale isn't held steady.
    morale_free: i64,
    /// `CanHaveDisasters`'s answer (1: disasters on).
    disasters: i64,
}

impl Env {
    /// A lobby key's value; None for a player the lobby doesn't have, a draw or the loop's iteration.
    fn get(&self, k: &Key) -> Option<i64> {
        match k {
            Key::N => Some(self.n),
            Key::Eden(p) => self.eden.get(*p).copied(),
            Key::Level(p) => self.level.get(*p).copied(),
            Key::Units => Some(self.units),
            Key::DayNight => Some(self.day_night),
            Key::MoraleFree => Some(self.morale_free),
            Key::Disasters => Some(self.disasters),
            Key::Var(_) | Key::Iter => None,
        }
    }
}

/// What a value of the Lua can follow: a lobby input or an earlier draw (`R[i]`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    /// The human players.
    N,
    /// A player's colony (0-based player; 1: Eden).
    Eden(usize),
    /// A player's resource level (0-based player; 0 to 2).
    Level(usize),
    /// The initial vehicles.
    Units,
    /// The lobby's flags, as [`Env`] holds them.
    DayNight,
    MoraleFree,
    Disasters,
    /// An earlier draw, `R[i]`.
    Var(usize),
    /// The iteration of the loop the calls are in (0 first): which initial vehicle.
    Iter,
}

/// The Lua expression for a key's value (flags as 0 or 1, players 1-based in `players`).
fn key_lua(k: &Key) -> String {
    match k {
        Key::N => "n".into(),
        Key::Eden(p) => format!("(players[{}].is_eden and 1 or 0)", p + 1),
        Key::Level(p) => format!("LEVEL_OF[players[{}].difficulty]", p + 1),
        Key::Units => "game.initial_units()".into(),
        Key::DayNight => "(game.uses_day_night() and 1 or 0)".into(),
        Key::MoraleFree => "(game.uses_morale() and 1 or 0)".into(),
        Key::Disasters => "(game.disasters_enabled() and 1 or 0)".into(),
        Key::Var(i) => format!("R[{i}]"),
        Key::Iter => "(i - 1)".into(),
    }
}

/// A field of a call, as the Lua writes it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Val {
    /// A number: also a player or a unit counted back (see [`Kind`]).
    Int(i64),
    /// A name or text, quoted when written.
    Text(String),
}

/// How a field is written: a number, a string, a unit the setup made (how many units ago), a player (0-based).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    /// A number, written as it is or as the expression it follows.
    Num,
    /// A string.
    Text,
    /// A unit the setup made, as how many units back from the latest (0: the latest, `U[#U]`).
    UnitBack,
    /// A player, 0-based (`players[p + 1]` in the Lua).
    Player,
}

/// One step of a setup's run: what the replay aligns across runs.
#[derive(Clone, Debug)]
enum Tok {
    /// Consecutive draws of one range.
    Draws { range: i64, vals: Vec<i64> },
    /// A call: what the Lua makes (`api` is the Lua form's name), its fields.
    Act { api: &'static str, kinds: Vec<Kind>, vals: Vec<Val> },
    /// The setup's first trigger: the replay ends here.
    End,
    /// A block made once for each of the lobby's initial vehicles (a draw of where, then the vehicle): its iterations.
    Loop(Vec<Vec<Tok>>),
}

/// A token without its values: what runs are aligned on.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Shape {
    /// Draws of a range, however many.
    Draws(i64),
    /// A call; a unit's with its type (so units of a type line up: a structure's calls aren't a vehicle's).
    Act(&'static str, Vec<Kind>, Option<Val>),
    /// The setup's first trigger.
    End,
    /// A loop, by its first iteration's shapes.
    Loop(Vec<Shape>),
}

/// A token's shape.
fn shape(t: &Tok) -> Shape {
    match t {
        Tok::Draws { range, .. } => Shape::Draws(*range),
        Tok::Act { api, kinds, vals } => Shape::Act(api, kinds.clone(), (*api == "unit").then(|| vals[0].clone())),
        Tok::End => Shape::End,
        Tok::Loop(iters) => Shape::Loop(iters.first().map_or(Vec::new(), |b| b.iter().map(shape).collect())),
    }
}

/// `toks` with the block made once per initial vehicle as a loop: from a draw, the stretch up to the next draw of its
/// range, repeated as many times as the lobby's initial vehicles, each alike in shape. Later blocks are looked for after
/// the first; `toks` unchanged when there is none.
fn loops(toks: Vec<Tok>, units: usize) -> Vec<Tok> {
    if units == 0 {
        return toks;
    }
    for i in 0..toks.len() {
        let Tok::Draws { range, .. } = &toks[i] else { continue };
        // The block's length: up to the next draw of this range.
        let Some(p) = toks[i + 1..].iter().position(|t| matches!(t, Tok::Draws { range: r, .. } if r == range)).map(|p| p + 1) else {
            continue;
        };
        if i + p * units > toks.len() {
            continue;
        }
        let block: Vec<Shape> = toks[i..i + p].iter().map(shape).collect();
        let alike = (1..units).all(|j| toks[i + j * p..i + (j + 1) * p].iter().map(shape).collect::<Vec<_>>() == block);
        // a vehicle in each, and no more of the same block after
        let has_unit = block.iter().any(|s| matches!(s, Shape::Act("unit", ..)));
        let more = i + (units + 1) * p <= toks.len()
            && toks[i + units * p..i + (units + 1) * p].iter().map(shape).collect::<Vec<_>>() == block;
        // the same players in each: a block made once per player (as many players as initial vehicles) isn't one
        let players = |b: &[Tok]| -> BTreeSet<i64> {
            b.iter()
                .flat_map(|t| match t {
                    Tok::Act { kinds, vals, .. } => kinds
                        .iter()
                        .zip(vals)
                        .filter_map(|(k, v)| if let (Kind::Player, Val::Int(p)) = (k, v) { Some(*p) } else { None })
                        .collect::<Vec<_>>(),
                    _ => Vec::new(),
                })
                .collect()
        };
        let same_players = (1..units).all(|j| players(&toks[i + j * p..i + (j + 1) * p]) == players(&toks[i..i + p]));
        if alike && has_unit && !more && same_players {
            let iters: Vec<Vec<Tok>> = (0..units).map(|j| toks[i + j * p..i + (j + 1) * p].to_vec()).collect();
            let mut out: Vec<Tok> = toks[..i].to_vec();
            out.push(Tok::Loop(iters));
            out.extend(loops(toks[i + units * p..].to_vec(), units));
            return out;
        }
    }
    toks
}

/// `toks` with each run of wide draws (a percentage) as one draw per token; narrow ones, picks of a few spots, stay runs.
fn singles(toks: &[Tok]) -> Vec<Tok> {
    toks.iter()
        .flat_map(|t| match t {
            Tok::Draws { range, vals } if *range >= 64 => vals.iter().map(|v| Tok::Draws { range: *range, vals: vec![*v] }).collect(),
            t => vec![t.clone()],
        })
        .collect()
}

/// One game of the setup: its lobby and its run as tokens.
struct Sample {
    env: Env,
    /// The run as [`tokens`] makes it.
    toks: Vec<Tok>,
    /// The same with the block made once per initial vehicle as a loop.
    looped: Vec<Tok>,
}

/// The arguments of a call, split at top-level commas.
fn split(c: &CallRec) -> Vec<String> {
    let all = c.args.join(", ");
    let (mut out, mut depth, mut cur) = (Vec::new(), 0, String::new());
    for ch in all.chars() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            _ => {}
        }
        if ch == ',' && depth == 0 {
            out.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(ch);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// A logged argument as a number (`this=` dropped).
fn num(s: &str) -> Option<i64> {
    s.trim().trim_start_matches("this=").parse().ok()
}

/// The number of a logged handle such as `Unit#12` or `Player3`, by its `prefix`.
fn hash_num(s: &str, prefix: &str) -> Option<i64> {
    s.trim().trim_start_matches("this=").strip_prefix(prefix)?.parse().ok()
}

/// A logged `(x,y)`.
fn pair(s: &str) -> Option<(i64, i64)> {
    let (a, b) = s.trim().trim_start_matches('(').trim_end_matches(')').split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// The lobby's answer to a lobby question: None when `api` isn't one, `Some(None)` when the lobby has no answer (a
/// player it doesn't have). `tokens` checks the logged answer against it, so a run that disagrees with its lobby stops.
/// The game's player count includes a computer player (`ai`), after the humans.
fn ask(api: &str, args: &[String], env: &Env, ai: bool) -> Option<Option<i64>> {
    let p = || args.first().and_then(|a| hash_num(a, "Player")).map(|p| p as usize);
    Some(match api {
        "TethysGame::NoPlayers" => Some(env.n + i64::from(ai)),
        "TethysGame::InitialUnits" => Some(env.units),
        "TethysGame::UsesDayNight" => Some(env.day_night),
        "TethysGame::UsesMorale" => Some(env.morale_free),
        "TethysGame::CanHaveDisasters" => Some(env.disasters),
        "_Player::IsEden" => p().and_then(|p| env.eden.get(p).copied()),
        // A computer player's level is the host's (Peacekeeper's setup asks it to set the AI's colonists).
        "_Player::Difficulty" => p().and_then(|p| {
            env.level.get(p).copied().or_else(|| (ai && p as i64 == env.n).then(|| env.level.first().copied()).flatten())
        }),
        "_Player::IsHuman" => p().map(|p| i64::from((p as i64) < env.n)),
        _ => return None,
    })
}

/// Calls (by the end of their name) that end the replay: triggers and AI groups.
const TRIGGERS: &[&str] = &[
    "CreateTimeTrigger", "CreateVictoryCondition", "CreateFailureCondition", "CreateOnePlayerLeftTrigger",
    "CreateCountTrigger", "CreateResourceTrigger", "CreateVehicleCountTrigger", "CreateOperationalTrigger",
    "CreateSetTrigger", "CreateResearchTrigger", "CreateMidasTrigger", "CreateBuildingCountTrigger",
    "CreateFightGroup", "CreateMiningGroup", "CreateBuildingGroup",
];

/// A setup's `InitProc` calls as tokens up to its first trigger, or why they can't be: a call it doesn't understand, a
/// cheat turned on, or a lobby answer that isn't the lobby's. Tiles come out as the Lua's (x less `pad`, 1-based).
///
/// With a computer player (`ai`, the player after the humans), its setup is `ai.rs`'s `setup_ai`: from its first
/// group or unit up to the setup's first draw (its base, groups and fixed-time triggers, which draw nothing in the
/// stock maps: Peacekeeper, La Corrida) is one `ai` token where `setup_ai` goes, and the replay goes on after it.
fn tokens(calls: &[CallRec], env: &Env, pad: i64, ai: bool) -> Result<Vec<Tok>, String> {
    let mut toks: Vec<Tok> = Vec::new();
    // Where the computer player's setup is: not yet, being skipped, or done.
    let (mut in_ai, mut ai_done) = (false, !ai);
    // The units the setup made, in order, by the id the game gave each (the next call's handle names it).
    let mut made: BTreeMap<i64, usize> = BTreeMap::new();
    // The unit just made whose id isn't known yet.
    let mut pending: Option<usize> = None;
    // Units made so far.
    let mut count = 0usize;
    // Whether the one `lava` token is out yet.
    let mut lava = false;
    // A logged game x as the Lua's x: less the padding, 1-based (y only gets the +1).
    let tx = |x: i64| x - pad + 1;
    for c in calls.iter().filter(|c| c.phase == "InitProc") {
        let api = c.api.as_str();
        let a = split(c);
        if in_ai {
            // Its tubes and walls are the replay's, as every wall is (the placement makes none): Peacekeeper's
            // computer base gets a tube that `setup_ai` doesn't lay.
            if api == "TethysGame::GetRand" {
                in_ai = false;
                ai_done = true;
            } else if api != "TethysGame::CreateWallOrTube" {
                continue;
            }
        } else if !ai_done
            && ((api.ends_with("Group") && api.starts_with("Create"))
                || (api == "TethysGame::CreateUnit" && a.get(3).and_then(|o| num(o)) == Some(env.n)))
        {
            // The computer player's setup starts at its first group, or its first unit (owner `n`, the player after
            // the humans).
            toks.push(Tok::Act { api: "ai", kinds: Vec::new(), vals: Vec::new() });
            in_ai = true;
            continue;
        }
        // The first unit handle not seen before, in any call after a CreateUnit, is the unit it made.
        if let Some(p) = pending
            && let Some(id) = a.iter().find_map(|s| hash_num(s, "Unit#"))
            && id != 0
            && !made.contains_key(&id)
        {
            made.insert(id, p);
            pending = None;
        }
        if TRIGGERS.iter().any(|t| api.ends_with(t)) {
            toks.push(Tok::End);
            break;
        }
        if let Some(ans) = ask(api, &a, env, ai) {
            let ret = c.ret.trim().parse::<i64>().ok();
            if ans.is_none() || ret != ans {
                return Err(format!("{api} answered {} where the lobby says {ans:?}", c.ret));
            }
            continue;
        }
        // A unit handle as how many units back from the latest made (the Lua's `U[#U - k]`).
        let unit_back = |s: &str| -> Option<i64> {
            let id = hash_num(s, "Unit#")?;
            let k = *made.get(&id)?;
            Some((count - 1 - k) as i64)
        };
        let act = |api: &'static str, fields: Vec<(Kind, Val)>| -> Tok {
            let (kinds, vals) = fields.into_iter().unzip();
            Tok::Act { api, kinds, vals }
        };
        let n = |v: i64| (Kind::Num, Val::Int(v));
        let t = |s: String| (Kind::Text, Val::Text(s));
        let bad = || format!("{api}({}) isn't understood", c.args.join(", "));
        match api {
            "TethysGame::GetRand" => {
                let range = a.first().and_then(|s| num(s)).ok_or_else(bad)?;
                let v = c.ret.trim().parse::<i64>().map_err(|_| bad())?;
                match toks.last_mut() {
                    Some(Tok::Draws { range: r, vals }) if *r == range => vals.push(v),
                    _ => toks.push(Tok::Draws { range, vals: vec![v] }),
                }
            }
            "TethysGame::CreateUnit" => {
                // "Unit#n, TYPE, (x,y), owner, WEAPON|None, rotation"
                let [_, ty, xy, owner, weapon, rot] = &a[..] else { return Err(bad()) };
                let (x, y) = pair(xy).ok_or_else(bad)?;
                let w = if weapon == "None" { String::new() } else { lua_name(map_id(weapon)) };
                toks.push(act("unit", vec![
                    t(lua_name(map_id(ty))), n(tx(x)), n(y + 1), (Kind::Player, Val::Int(num(owner).ok_or_else(bad)?)), t(w),
                    n(num(rot).ok_or_else(bad)?.rem_euclid(8)),
                ]));
                pending = Some(count);
                count += 1;
            }
            "Unit::DoSetLights" | "Unit::SetFactoryCargo" | "Unit::SetTruckCargo" => {
                let u = a.first().and_then(|s| unit_back(s)).ok_or_else(bad)?;
                let back = (Kind::UnitBack, Val::Int(u));
                toks.push(match api {
                    "Unit::DoSetLights" => act("lights", vec![back, n(num(&a[1]).ok_or_else(bad)?)]),
                    "Unit::SetFactoryCargo" => {
                        let [_, bay, kit, weapon] = &a[..] else { return Err(bad()) };
                        let w = if weapon == "None" { String::new() } else { lua_name(map_id(weapon)) };
                        act("factory", vec![back, n(num(bay).ok_or_else(bad)? + 1), t(lua_name(map_id(kit))), t(w)])
                    }
                    _ => {
                        let [_, cargo, amount] = &a[..] else { return Err(bad()) };
                        let c = num(cargo).ok_or_else(bad)?;
                        let name = if c == 0 { String::new() } else { cargo_name(c as i32).ok_or_else(bad)?.to_string() };
                        act("cargo", vec![back, t(name), n(num(amount).ok_or_else(bad)?)])
                    }
                });
            }
            "TethysGame::CreateBeacon" => {
                // "TYPE, x, y, ore, bar, variant"
                let [ty, x, y, ore, bar, variant] = &a[..] else { return Err(bad()) };
                let (x, y, ore, bar, variant) = (num(x), num(y), num(ore), num(bar), num(variant));
                let (Some(x), Some(y), Some(ore), Some(bar), Some(variant)) = (x, y, ore, bar, variant) else { return Err(bad()) };
                let kind = match ty.as_str() { "MAGMA_VENT" => "MagmaVent", "FUMAROLE" => "Fumarole", _ => "MiningBeacon" };
                // The game's numbers for ore and yield (bar 0 is the 3-bar yield); anything else is random.
                let ore = match ore { 0 => "Common", 1 => "Rare", _ => "Random" };
                let yield_ = match bar { 0 => "Bar3", 1 => "Bar2", 2 => "Bar1", _ => "Random" };
                toks.push(act("beacon", vec![
                    t(kind.into()), t(ore.into()), t(yield_.into()), t(variant_name(bar, variant).into()), n(tx(x)), n(y + 1),
                ]));
            }
            "TethysGame::CreateWallOrTube" => {
                let [x, y, _, ty] = &a[..] else { return Err(bad()) };
                let (Some(x), Some(y)) = (num(x), num(y)) else { return Err(bad()) };
                let kind = match ty.as_str() { "WALL" => "Wall", "WALL_LAVA" => "LavaWall", "WALL_MICROBE" => "MicrobeWall", "TUBE" => "Tube", _ => return Err(bad()) };
                toks.push(act("wall", vec![t(kind.into()), n(tx(x)), n(y + 1)]));
            }
            "_Player::MarkResearchComplete" | "_Player::SetWorkers" | "_Player::SetScientists" | "_Player::SetKids"
            | "_Player::SetFoodStored" | "_Player::SetOre" | "_Player::SetRareOre" | "_Player::SetTechLevel" => {
                let [p, v] = &a[..] else { return Err(bad()) };
                let p = hash_num(p, "Player").ok_or_else(bad)?;
                let name = match api {
                    "_Player::MarkResearchComplete" => "research",
                    "_Player::SetWorkers" => "workers",
                    "_Player::SetScientists" => "scientists",
                    "_Player::SetKids" => "kids",
                    "_Player::SetFoodStored" => "food",
                    "_Player::SetOre" => "ore",
                    "_Player::SetRareOre" => "rare_ore",
                    _ => "tech_level",
                };
                toks.push(act(name, vec![(Kind::Player, Val::Int(p)), n(num(v).ok_or_else(bad)?)]));
            }
            "_Player::CenterViewOn" => {
                let [p, x, y] = &a[..] else { return Err(bad()) };
                let (Some(p), Some(x), Some(y)) = (hash_num(p, "Player"), num(x), num(y)) else { return Err(bad()) };
                toks.push(act("view", vec![(Kind::Player, Val::Int(p)), n(tx(x)), n(y + 1)]));
            }
            "_Player::GoEden" | "_Player::GoPlymouth" => {
                let p = a.first().and_then(|s| hash_num(s, "Player")).ok_or_else(bad)?;
                toks.push(act(if api.ends_with("GoEden") { "go_eden" } else { "go_plymouth" }, vec![(Kind::Player, Val::Int(p))]));
            }
            "_Player::AllyWith" => {
                let [p, q] = &a[..] else { return Err(bad()) };
                let (Some(p), Some(q)) = (hash_num(p, "Player"), num(q).or_else(|| hash_num(q, "Player"))) else { return Err(bad()) };
                toks.push(act("ally", vec![(Kind::Player, Val::Int(p)), (Kind::Player, Val::Int(q))]));
            }
            "GameMap::InitialSetTile" | "GameMap::SetCellType" => {
                let [xy, v] = &a[..] else { return Err(bad()) };
                let (x, y) = pair(xy).ok_or_else(bad)?;
                let v = num(v).ok_or_else(bad)?;
                toks.push(act(if api.ends_with("InitialSetTile") { "set_tile" } else { "cell_type" }, vec![n(tx(x)), n(y + 1), n(v)]));
            }
            "GameMap::SetLavaPossible" => {
                // Where lava may flow, all at once where the setup first marks a tile (they make nothing).
                if !lava {
                    lava = true;
                    toks.push(act("lava", vec![]));
                }
            }
            "TethysGame::SetDaylightEverywhere" | "TethysGame::SetDaylightMoves" => {
                let v = a.first().and_then(|s| num(s)).ok_or_else(bad)?;
                toks.push(act(if api.ends_with("Everywhere") { "daylight" } else { "daylight_moves" }, vec![n(v)]));
            }
            "TethysGame::ForceMoraleGreat" | "TethysGame::ForceMoraleGood" | "TethysGame::ForceMoraleOK"
            | "TethysGame::ForceMoralePoor" | "TethysGame::ForceMoraleRotten" | "TethysGame::FreeMoraleLevel" => {
                let level = match api {
                    "TethysGame::ForceMoraleGreat" => "Excellent",
                    "TethysGame::ForceMoraleGood" => "Good",
                    "TethysGame::ForceMoraleOK" => "Fair",
                    "TethysGame::ForceMoralePoor" => "Poor",
                    "TethysGame::ForceMoraleRotten" => "Terrible",
                    _ => "",
                };
                let p = a.first().and_then(|s| num(s)).ok_or_else(bad)?;
                toks.push(act(if level.is_empty() { "free_morale" } else { "morale" }, vec![t(level.into()), n(p)]));
            }
            "TethysGame::SetCheatFastUnits" | "TethysGame::SetCheatFastProduction" | "TethysGame::SetCheatUnlimitedResources"
            | "TethysGame::SetCheatProduceAll" => {
                // A cheat turned off changes nothing; one turned on isn't replayed.
                if a.first().and_then(|s| num(s)) != Some(0) {
                    return Err(bad());
                }
            }
            "TethysGame::CreateWreck" => {
                let [x, y, tech, disc] = &a[..] else { return Err(bad()) };
                let tech = num(tech).or_else(|| num(tech.trim().strip_prefix("map_id(")?.strip_suffix(')')?));
                let (Some(x), Some(y), Some(tech), Some(disc)) = (num(x), num(y), tech, num(disc)) else { return Err(bad()) };
                toks.push(act("wreck", vec![n(tx(x)), n(y + 1), n(tech), n(disc)]));
            }
            _ => return Err(bad()),
        }
    }
    Ok(toks)
}

/// The Lua of one call (`api` as `tokens` names it), its fields as Lua expressions.
fn act_lua(api: &str, f: &[String]) -> String {
    // An on/off field: `false` or `true` for a number, else the expression compared with 0.
    let flag = |s: &String| match s.parse::<i64>() {
        Ok(0) => "false".to_string(),
        Ok(_) => "true".to_string(),
        Err(_) => format!("{s} ~= 0"),
    };
    // A player field: a 0-based number, or an expression that gives one.
    let pl = |s: &str| format!("players[{s} + 1]");
    let pl = |s: &String| match s.parse::<i64>() {
        Ok(v) => format!("players[{}]", v + 1),
        Err(_) => pl(s),
    };
    // A unit field: how many units back from the latest made.
    let unit = |s: &String| match s.parse::<i64>() {
        Ok(0) => "U[#U]".to_string(),
        Ok(v) => format!("U[#U - {v}]"),
        Err(_) => format!("U[#U - ({s})]"),
    };
    // An optional named field, left out when it's the empty string.
    let opt = |name: &str, s: &String| if s == "\"\"" { String::new() } else { format!(", {name} = {s}") };
    let dir = |s: &String| match s.parse::<usize>() {
        Ok(d) => quoted(DIRECTIONS[d % 8]),
        Err(_) => format!("DIRS[{s} + 1]"),
    };
    match api {
        "unit" => format!(
            "U[#U + 1] = game.create_unit{{ type = {}, player = {}, at = {{ {}, {} }}{}, direction = {}, lights = false, exact = true }}[1]",
            f[0], pl(&f[3]), f[1], f[2], opt("weapon", &f[4]), dir(&f[5])
        ),
        "lights" => format!("{}:set_lights({})", unit(&f[0]), flag(&f[1])),
        "factory" => {
            if f[3] == "\"\"" { format!("{}:set_factory_cargo({}, {})", unit(&f[0]), f[1], f[2]) } else { format!("{}:set_factory_cargo({}, {}, {})", unit(&f[0]), f[1], f[2], f[3]) }
        }
        "cargo" => format!("{}:set_cargo({}, {})", unit(&f[0]), if f[1] == "\"\"" { "nil".into() } else { f[1].clone() }, f[2]),
        "beacon" => format!("game.create_beacon{{ type = {}, ore = {}, yield = {}, variant = {}, at = {{ {}, {} }} }}", f[0], f[1], f[2], f[3], f[4], f[5]),
        "wall" => match f[0].as_str() {
            "\"Tube\"" => format!("game.create_tube({{ {}, {} }})", f[1], f[2]),
            t if t.starts_with('"') => format!("game.create_wall({t}, {{ {}, {} }})", f[1], f[2]),
            t => format!("if {t} == \"Tube\" then game.create_tube({{ {x}, {y} }}) else game.create_wall({t}, {{ {x}, {y} }}) end", x = f[1], y = f[2]),
        },
        "research" => format!("{}:mark_research({})", pl(&f[0]), f[1]),
        "workers" | "scientists" | "kids" | "food" | "ore" | "rare_ore" => format!("{}.{api} = {}", pl(&f[0]), f[1]),
        "tech_level" => format!("{}:set_tech_level({})", pl(&f[0]), f[1]),
        "view" => format!("{}:center_view({{ {}, {} }})", pl(&f[0]), f[1], f[2]),
        "go_eden" => format!("{}:go_eden()", pl(&f[0])),
        "go_plymouth" => format!("{}:go_plymouth()", pl(&f[0])),
        "ally" => format!("{}:ally_with({})", pl(&f[0]), pl(&f[1])),
        "set_tile" => format!("game.initial_set_tile({{ {}, {} }}, {})", f[0], f[1], f[2]),
        "cell_type" => format!("game.set_cell_type({{ {}, {} }}, {})", f[0], f[1], f[2]),
        "lava" => "for _, area in ipairs(LAVA_POSSIBLE) do game.set_lava_possible(area) end".into(),
        "ai" => "setup_ai(players[n + 1])".into(),
        "daylight" => format!("game.set_daylight_everywhere({})", flag(&f[0])),
        "daylight_moves" => format!("game.set_daylight_moves({})", flag(&f[0])),
        "morale" => {
            if f[1] == "-1" { format!("game.morale_steady({})", f[0]) } else { format!("game.morale_steady({}, {})", f[0], pl(&f[1])) }
        }
        "free_morale" => if f[1] == "-1" { "game.free_morale()".into() } else { format!("game.free_morale({})", pl(&f[1])) },
        "wreck" => format!("game.create_wreckage({{ {}, {} }}, {}, {})", f[0], f[1], f[2], flag(&f[3])),
        _ => format!("-- {api}"),
    }
}

/// A constant field as Lua.
fn lit(v: &Val) -> String {
    match v {
        Val::Int(i) => i.to_string(),
        Val::Text(s) => quoted(s),
    }
}

/// The draws the Lua keeps (`R`), each with its value in the samples that make it.
struct Vars {
    /// Each draw's value by sample (a sample whose run didn't make it has none).
    vals: Vec<BTreeMap<usize, i64>>,
    /// Each draw's range.
    range: Vec<i64>,
    /// How many draws of each range each sample has made so far, by (sample, range).
    count: BTreeMap<(usize, i64), usize>,
    /// The `R` of each role, by (scope, range, draws of that range before it). A draw's role is its range and how many
    /// of that range came before it in its game, so the same draw made in two branches (a player's start, after
    /// initial vehicles in one and none in the other) is one `R` the setup's later choices follow.
    role: BTreeMap<(usize, i64, usize), usize>,
    /// The loop the draws being made are in (0: none), so a loop's draws don't take the roles of the setup's.
    scope: usize,
}

/// A draw each sample of `vals` makes (sample to value): its `R` index. When every sample gives it the same role and
/// that role's `R` has no value yet for these samples, it is that `R`; otherwise a new one.
fn new_var(vars: &mut Vars, range: i64, vals: BTreeMap<usize, i64>) -> usize {
    let roles: BTreeSet<usize> = vals
        .keys()
        .map(|s| {
            let c = vars.count.entry((*s, range)).or_insert(0);
            *c += 1;
            *c - 1
        })
        .collect();
    let role = (roles.len() == 1).then(|| (vars.scope, range, *roles.iter().next().expect("one")));
    if let Some(i) = role.and_then(|r| vars.role.get(&r).copied())
        && vals.keys().all(|s| !vars.vals[i].contains_key(s))
    {
        vars.vals[i].extend(vals);
        return i;
    }
    let i = vars.vals.len();
    vars.vals.push(vals);
    vars.range.push(range);
    if let Some(r) = role {
        vars.role.entry(r).or_insert(i);
    }
    i
}

/// One attempt at writing the replay: the samples, the draws so far and the Lua written.
struct Ctx<'a> {
    /// The games the replay is written from.
    samples: &'a [Sample],
    /// The draws kept so far.
    vars: Vars,
    /// The Lua of `on_init` so far.
    out: String,
    /// Whether the Lua uses `pick` and `draw_avoid`, so the head defines them.
    picks: bool,
    avoids: bool,
    /// Each sample id's game: its own for a game, the game for an iteration of a loop (ids past the games are the
    /// loops' iterations, added as loops are written).
    origin: Vec<usize>,
    /// An iteration's index in its loop (None for a game).
    iter: Vec<Option<i64>>,
}

/// The keys every sample of `items` has a value for, most telling first: the latest draws, then the lobby's (the
/// `prefer` player's colony and level first), then the loop's iteration when every sample is in a loop.
fn keys(ctx: &Ctx, items: &[(usize, &[Tok])], prefer: Option<usize>) -> Vec<Key> {
    let mut ks: Vec<Key> = (0..ctx.vars.vals.len())
        .rev()
        .filter(|i| items.iter().all(|(s, _)| var_val(ctx, *i, *s).is_some()))
        .map(Key::Var)
        .collect();
    ks.push(Key::N);
    let maxn = items.iter().map(|(s, _)| ctx.samples[ctx.origin[*s]].env.n).min().unwrap_or(0) as usize;
    // The player the calls here are about first: the samples give every player the host's resource level, so any
    // player's level decides alike in them, and the right one is that player's.
    if let Some(p) = prefer.filter(|p| *p < maxn) {
        ks.push(Key::Level(p));
        ks.push(Key::Eden(p));
    }
    for p in 0..maxn {
        ks.push(Key::Eden(p));
        ks.push(Key::Level(p));
    }
    ks.extend([Key::Units, Key::DayNight, Key::MoraleFree, Key::Disasters]);
    if items.iter().all(|(s, _)| ctx.iter.get(*s).copied().flatten().is_some()) {
        ks.push(Key::Iter);
    }
    ks
}

/// A key's value in sample `s`: a draw's, the loop's iteration, or its game's lobby's.
fn key_val(ctx: &Ctx, k: &Key, s: usize) -> Option<i64> {
    match k {
        Key::Var(i) => var_val(ctx, *i, s),
        Key::Iter => ctx.iter.get(s).copied().flatten(),
        _ => ctx.samples[ctx.origin[s]].env.get(k),
    }
}

/// A draw's value for sample `s`: its own, or (an iteration of a loop) its game's.
fn var_val(ctx: &Ctx, i: usize, s: usize) -> Option<i64> {
    ctx.vars.vals[i].get(&s).or_else(|| ctx.vars.vals[i].get(&ctx.origin[s])).copied()
}

/// A Lua expression for a value that each sample of `items` has (`get`): a constant; else the one key it follows, by a
/// line (`a + b * key`) or a table, the best by `best_key`; else a line in one key plus an offset another decides; else a
/// table of two keys. An error when none fits.
fn expr(ctx: &Ctx, items: &[(usize, &[Tok])], prefer: Option<usize>, get: &dyn Fn(usize) -> Val) -> Result<String, String> {
    let vals: Vec<(usize, Val)> = items.iter().map(|(s, _)| (*s, get(*s))).collect();
    if vals.iter().all(|(_, v)| *v == vals[0].1) {
        return Ok(lit(&vals[0].1));
    }
    // Each key that decides the value (one value for each of its own): the key, how many of its values the samples
    // cover, the expression.
    let mut found: Vec<(Key, usize, String)> = Vec::new();
    for k in keys(ctx, items, prefer) {
        // The key's value to the field's; a key that gives one value two fields doesn't decide it.
        let mut table: BTreeMap<i64, Val> = BTreeMap::new();
        let mut ok = true;
        for (s, v) in &vals {
            let Some(kv) = key_val(ctx, &k, *s) else { ok = false; break };
            match table.get(&kv) {
                Some(w) if w != v => {
                    ok = false;
                    break;
                }
                _ => {
                    table.insert(kv, v.clone());
                }
            }
        }
        if !ok || table.len() < 2 {
            continue;
        }
        let kl = key_lua(&k);
        // A number the key gives by `a + b * key`, for keys that never came up too (Land Rush's start tiles).
        let ints: Vec<(i64, i64)> = table.iter().filter_map(|(k, v)| if let Val::Int(i) = v { Some((*k, *i)) } else { None }).collect();
        // Three values at least: two always fit a line.
        if ints.len() == table.len() && ints.len() >= 3 {
            let (k0, v0) = ints[0];
            let (k1, v1) = ints[1];
            if k1 != k0 && (v1 - v0) % (k1 - k0) == 0 {
                let b = (v1 - v0) / (k1 - k0);
                let a = v0 - b * k0;
                if ints.iter().all(|(k, v)| a + b * k == *v) {
                    // A line covers the values that never came up too, so it counts as covering the key's domain.
                    let d = domain(ctx, &k) as usize;
                    found.push((k, d, match (a, b) {
                        (0, 1) => kl,
                        (a, 1) => format!("{kl} + {a}").replace("+ -", "- "),
                        (a, b) => format!("{a} + {b} * {kl}").replace("+ -", "- "),
                    }));
                    continue;
                }
            }
        }
        // A wide draw's table (a range over 32; Flood Plain's wreck: a spot on a circle by an angle of 360) with values no
        // sampled game drew: when the samples cover three quarters of the range at least, each missing value whose
        // nearest seen neighbours on both sides (wrapping round) agree gets their value. Coverage counts only the values
        // seen.
        let seen = table.len();
        let d = domain(ctx, &k);
        if matches!(k, Key::Var(_)) && d > 32 && table.len() as i64 * 4 >= d * 3 {
            let near = |from: i64, step: i64| (1..d).map(|i| (from + step * i).rem_euclid(d)).find_map(|x| table.get(&x).cloned());
            let fill: Vec<(i64, Val)> = (0..d)
                .filter(|x| !table.contains_key(x))
                .filter_map(|x| match (near(x, -1), near(x, 1)) {
                    (Some(a), Some(b)) if a == b => Some((x, a)),
                    _ => None,
                })
                .collect();
            table.extend(fill);
        }
        let entries: Vec<String> = table.iter().map(|(k, v)| format!("[{k}] = {}", lit(v))).collect();
        found.push((k, seen, format!("({{ {} }})[{kl}]", entries.join(", "))));
    }
    if let Some((_, e)) = best_key(ctx, found) {
        return Ok(e);
    }
    let ks = keys(ctx, items, prefer);
    // A line in one key plus an offset another decides (a Land Rush unit's tile: the start drawn, plus where its column
    // puts it, by the way it faces). The line's key has over 16 values, the offset's 16 or fewer; the slope is the same
    // for every offset and not 0.
    if vals.iter().all(|(_, v)| matches!(v, Val::Int(_))) {
        let num = |s: usize| -> i64 { match vals.iter().find(|(x, _)| *x == s).map(|(_, v)| v) { Some(Val::Int(i)) => *i, _ => 0 } };
        for k1 in ks.iter().filter(|k| domain(ctx, k) > 16) {
            'k2: for k2 in ks.iter().filter(|k| *k != k1 && domain(ctx, k) <= 16) {
                let mut slope: Option<i64> = None;
                let mut by: BTreeMap<i64, Vec<(i64, i64)>> = BTreeMap::new();
                for (s, _) in items {
                    let (Some(a), Some(b)) = (key_val(ctx, k1, *s), key_val(ctx, k2, *s)) else { continue 'k2 };
                    by.entry(b).or_default().push((a, num(*s)));
                }
                for pts in by.values() {
                    if let (Some(p), Some(q)) = (pts.first(), pts.iter().find(|q| q.0 != pts[0].0)) {
                        if (q.1 - p.1) % (q.0 - p.0) != 0 {
                            continue 'k2;
                        }
                        let b = (q.1 - p.1) / (q.0 - p.0);
                        if slope.is_some_and(|x| x != b) {
                            continue 'k2;
                        }
                        slope = Some(b);
                    }
                }
                let Some(b) = slope.filter(|b| *b != 0) else { continue };
                let mut offset: BTreeMap<i64, i64> = BTreeMap::new();
                for (k2v, pts) in &by {
                    let c = pts[0].1 - b * pts[0].0;
                    if pts.iter().any(|(a, v)| v - b * a != c) {
                        continue 'k2;
                    }
                    offset.insert(*k2v, c);
                }
                let line = if b == 1 { key_lua(k1) } else { format!("{b} * {}", key_lua(k1)) };
                let table: Vec<String> = offset.iter().map(|(k, c)| format!("[{k}] = {c}")).collect();
                return Ok(format!("{line} + ({{ {} }})[{}]", table.join(", "), key_lua(k2)));
            }
        }
    }
    // Two keys together (a base's tile: the start a player got and the layout drawn there): the first pair, in `keys`
    // order, that decides the value.
    for (i, k1) in ks.iter().enumerate() {
        'k2: for k2 in &ks[i + 1..] {
            let mut table: BTreeMap<i64, BTreeMap<i64, Val>> = BTreeMap::new();
            for (s, v) in &vals {
                let (Some(a), Some(b)) = (key_val(ctx, k1, *s), key_val(ctx, k2, *s)) else { continue 'k2 };
                let row = table.entry(a).or_default();
                match row.get(&b) {
                    Some(w) if w != v => continue 'k2,
                    _ => {
                        row.insert(b, v.clone());
                    }
                }
            }
            let rows: Vec<String> = table
                .iter()
                .map(|(a, row)| format!("[{a}] = {{ {} }}", row.iter().map(|(b, v)| format!("[{b}] = {}", lit(v))).collect::<Vec<_>>().join(", ")))
                .collect();
            return Ok(format!("({{ {} }})[{}][{}]", rows.join(", "), key_lua(k1), key_lua(k2)));
        }
    }
    Err(format!("a value no draw or lobby input decides: {:?}", vals.iter().map(|(_, v)| v.clone()).collect::<BTreeSet<_>>()))
}

/// How many values a key can take: a draw's range, a lobby choice's (the player counts sampled for `n`; 12 iterations at
/// most for a loop).
fn domain(ctx: &Ctx, k: &Key) -> i64 {
    match k {
        Key::Var(i) => ctx.vars.range[*i],
        Key::N => ctx.samples.iter().map(|s| s.env.n).collect::<BTreeSet<_>>().len() as i64,
        Key::Level(_) => 3,
        Key::Units => 4,
        Key::Iter => 12,
        _ => 2,
    }
}

/// The key, of those that decide alike, whose values the samples cover best (a draw of 16 can tell a few dozen games
/// apart by chance; the draw that decides comes up with all its values), then a lobby input before a draw, then the one
/// with the fewest values, then the first. `found` holds each key with how many of its values the samples cover.
fn best_key<T>(ctx: &Ctx, found: Vec<(Key, usize, T)>) -> Option<(Key, T)> {
    found
        .into_iter()
        .enumerate()
        .min_by_key(|(i, (k, seen, _))| ((domain(ctx, k) - *seen as i64).max(0), i64::from(matches!(k, Key::Var(_))), domain(ctx, k), *i))
        .map(|(_, (k, _, t))| (k, t))
}

/// The key whose value decides each sample's `label` (each of its values gives one label), and its values by label;
/// None when no key does. The best by `best_key`.
fn decider(ctx: &Ctx, items: &[(usize, &[Tok])], label: &[usize], prefer: Option<usize>) -> Option<(Key, BTreeMap<usize, BTreeSet<i64>>)> {
    let mut found = Vec::new();
    'k: for k in keys(ctx, items, prefer) {
        let mut of: BTreeMap<i64, usize> = BTreeMap::new();
        let mut by: BTreeMap<usize, BTreeSet<i64>> = BTreeMap::new();
        for (i, (s, _)) in items.iter().enumerate() {
            let Some(v) = key_val(ctx, &k, *s) else { continue 'k };
            match of.get(&v) {
                Some(l) if *l != label[i] => continue 'k,
                _ => {
                    of.insert(v, label[i]);
                    by.entry(label[i]).or_default().insert(v);
                }
            }
        }
        let seen = of.len();
        found.push((k, seen, by));
    }
    best_key(ctx, found)
}

/// LCS of two shape lists: the matched index pairs.
fn lcs(a: &[Shape], b: &[Shape]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    // t[i][j]: the LCS length of a[i..] and b[j..]; then walked from the front, so ties keep the earlier match.
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

/// Writes the samples' token lists `items` (aligned from here on) as Lua at `ind`: token by token when all alike in
/// shape, else cut by `segments`, else aligned on anchors with the gaps between them written by `gap`.
fn emit(ctx: &mut Ctx, items: &[(usize, &[Tok])], ind: &str) -> Result<(), String> {
    if items.is_empty() || items.iter().all(|(_, t)| t.is_empty()) {
        return Ok(());
    }
    let shapes: Vec<Vec<Shape>> = items.iter().map(|(_, t)| t.iter().map(shape).collect()).collect();
    if shapes.iter().all(|s| *s == shapes[0]) {
        for i in 0..shapes[0].len() {
            let col: Vec<(usize, &[Tok])> = items.iter().map(|(s, t)| (*s, &t[i..=i])).collect();
            token(ctx, &col, ind)?;
        }
        return Ok(());
    }
    if let Some(parts) = segments(items, &shapes) {
        for part in &parts {
            emit(ctx, part, ind)?;
        }
        return Ok(());
    }
    // Anchors: the longest list's tokens every other list has, in order (its LCS with each, intersected); each anchor
    // holds the token's index in every list.
    let r = (0..items.len()).max_by_key(|i| shapes[*i].len()).unwrap_or(0);
    let mut common: Option<BTreeMap<usize, Vec<usize>>> = None;
    for (i, s) in shapes.iter().enumerate() {
        let m: BTreeMap<usize, usize> = if i == r { (0..s.len()).map(|k| (k, k)).collect() } else { lcs(&shapes[r], s).into_iter().collect() };
        common = Some(match common {
            None => m.into_iter().map(|(a, b)| (a, vec![b])).collect(),
            Some(c) => c
                .into_iter()
                .filter_map(|(a, mut v)| {
                    m.get(&a).map(|b| {
                        v.push(*b);
                        (a, v)
                    })
                })
                .collect(),
        });
    }
    let anchors: Vec<Vec<usize>> = common.unwrap_or_default().into_values().collect();
    // With OP2_REPLAY_DEBUG: the anchors, less the beacons, tiles and walls.
    if std::env::var("OP2_REPLAY_DEBUG").is_ok() {
        let line: Vec<String> = anchors
            .iter()
            .map(|a| match shape(&items[0].1[a[0]]) {
                Shape::Act(api, _, t) => format!("{api}{}", t.map_or(String::new(), |t| format!(":{t:?}"))),
                other => format!("{other:?}"),
            })
            .filter(|s| !s.starts_with("beacon") && !s.starts_with("set_tile") && !s.starts_with("wall"))
            .collect();
        eprintln!("{ind}{} items, {} anchors: {}", items.len(), anchors.len(), line.join(" | "));
    }
    // Each gap then its anchor, and the gap after the last anchor.
    let mut from = vec![0usize; items.len()];
    for a in anchors.iter().map(Some).chain(std::iter::once(None)) {
        let to: Vec<usize> = match a { Some(a) => a.clone(), None => items.iter().map(|(_, t)| t.len()).collect() };
        // The gap before the anchor: alike everywhere, or a branch.
        let gaps: Vec<(usize, &[Tok])> = items.iter().enumerate().map(|(i, (s, t))| (*s, &t[from[i]..to[i]])).collect();
        gap(ctx, &gaps, ind)?;
        if a.is_none() {
            break;
        }
        let col: Vec<(usize, &[Tok])> = items.iter().enumerate().map(|(i, (s, t))| (*s, &t[to[i]..=to[i]])).collect();
        token(ctx, &col, ind)?;
        from = to.iter().map(|k| k + 1).collect();
    }
    Ok(())
}

/// A sample's tokens from some point: its id and the tokens.
type Item<'a> = (usize, &'a [Tok]);

/// `items` cut where a stretch made once per player starts (a draw then a unit, or a unit then a draw, of the same shapes
/// and as many times in every sample: Olympus Mons' layout drawn, then the base's Command Center): what comes before the
/// first, then each stretch, so each player's base is decided by its own draws after the others', not inside their `if`s.
/// The pair must come twice at least. Failing that, cut where each run of walls starts; None when neither fits.
fn segments<'a>(items: &[Item<'a>], shapes: &[Vec<Shape>]) -> Option<Vec<Vec<Item<'a>>>> {
    let s0 = &shapes[0];
    let mut tried: BTreeSet<(Shape, Shape)> = BTreeSet::new();
    let unit = |s: &Shape| matches!(s, Shape::Act("unit", ..));
    let draw = |s: &Shape| matches!(s, Shape::Draws(_));
    // Pairs of the first sample tried: a draw and a unit first; then a draw and any call (Axen's Home: a start drawn,
    // then the base's walls). Each pair once.
    let pairs = 0..s0.len().saturating_sub(1);
    let marks = pairs
        .clone()
        .filter(|i| (draw(&s0[*i]) && unit(&s0[i + 1])) || (unit(&s0[*i]) && draw(&s0[i + 1])))
        .chain(pairs.filter(|i| draw(&s0[*i]) && matches!(s0[i + 1], Shape::Act(..))));
    for i in marks {
        let (a, b) = (&s0[i], &s0[i + 1]);
        if !tried.insert((a.clone(), b.clone())) {
            continue;
        }
        let at: Vec<Vec<usize>> = shapes.iter().map(|s| (0..s.len().saturating_sub(1)).filter(|j| s[*j] == *a && s[j + 1] == *b).collect()).collect();
        let k = at[0].len();
        if k < 2 || at.iter().any(|p| p.len() != k) {
            continue;
        }
        let mut parts: Vec<Vec<Item>> = vec![Vec::new(); k + 1];
        for ((s, t), p) in items.iter().zip(&at) {
            let mut cuts = vec![0];
            cuts.extend(p.iter().copied());
            cuts.push(t.len());
            for j in 0..=k {
                parts[j].push((*s, &t[cuts[j]..cuts[j + 1]]));
            }
        }
        return Some(parts);
    }
    // Where a run of walls starts, after something else and as many times in every sample (Root Canal: a player's base,
    // then the next player's walls, which the next start decides): what comes before each run, then each run on.
    let wall = |s: &Shape| matches!(s, Shape::Act("wall", ..));
    let at: Vec<Vec<usize>> = shapes.iter().map(|s| (1..s.len()).filter(|j| wall(&s[*j]) && !wall(&s[j - 1])).collect()).collect();
    let k = at[0].len();
    if k >= 1 && at.iter().all(|p| p.len() == k) && shapes.iter().all(|s| s.len() > 1) {
        let mut parts: Vec<Vec<Item>> = vec![Vec::new(); k + 1];
        for ((s, t), p) in items.iter().zip(&at) {
            let mut cuts = vec![0];
            cuts.extend(p.iter().copied());
            cuts.push(t.len());
            for j in 0..=k {
                parts[j].push((*s, &t[cuts[j]..cuts[j + 1]]));
            }
        }
        return Some(parts);
    }
    None
}

/// The one player the calls of `items` are about, if they're all about one.
fn one_player(items: &[(usize, &[Tok])]) -> Option<usize> {
    let players: BTreeSet<i64> = items
        .iter()
        .flat_map(|(_, t)| t.iter())
        .flat_map(|t| match t {
            Tok::Act { kinds, vals, .. } => kinds
                .iter()
                .zip(vals)
                .filter(|(k, _)| **k == Kind::Player)
                .filter_map(|(_, v)| if let Val::Int(p) = v { Some(*p) } else { None })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect();
    (players.len() == 1).then(|| *players.iter().next().expect("one") as usize)
}

/// What `branch` writes each group with, once one label is left.
type Leaf = dyn Fn(&mut Ctx, &[(usize, &[Tok])], &str) -> Result<(), String>;

/// `items` whose samples make different things (`labels`: the same label, the same things), each group by `leaf`: an
/// `if` on the key that decides the labels, or, when no one key does, on the key that splits them best, each group
/// decided in turn (each player's base by its own draw, colony and resource level). Every `if` ends in an `else` that
/// calls `unseen`. An error when no key splits them.
fn branch(ctx: &mut Ctx, items: &[(usize, &[Tok])], labels: &[usize], ind: &str, leaf: &Leaf) -> Result<(), String> {
    let kinds: BTreeSet<usize> = labels.iter().copied().collect();
    if kinds.len() == 1 {
        return leaf(ctx, items, ind);
    }
    let prefer = one_player(items);
    let inner = format!("{ind}  ");
    if let Some((k, by)) = decider(ctx, items, labels, prefer) {
        let kl = key_lua(&k);
        for (i, (l, vs)) in by.iter().enumerate() {
            let cond: Vec<String> = vs.iter().map(|v| format!("{kl} == {v}")).collect();
            let _ = writeln!(ctx.out, "{ind}{} {} then", if i == 0 { "if" } else { "elseif" }, cond.join(" or "));
            let sub: Vec<(usize, &[Tok])> = items.iter().zip(labels).filter(|(_, x)| *x == l).map(|(g, _)| *g).collect();
            leaf(ctx, &sub, &inner)?;
        }
        let _ = writeln!(ctx.out, "{ind}else
{ind}  unseen({}, {kl})
{ind}end", quoted(&kl));
        return Ok(());
    }
    // What they all make first, alike in shape, made before the `if` (a base's layout drawn, then the base it decides).
    if items.iter().all(|(_, t)| t.len() > 1) && items.iter().all(|(_, t)| shape(&t[0]) == shape(&items[0].1[0])) {
        let col: Vec<(usize, &[Tok])> = items.iter().map(|(s, t)| (*s, &t[..1])).collect();
        token(ctx, &col, ind)?;
        let rest: Vec<(usize, &[Tok])> = items.iter().map(|(s, t)| (*s, &t[1..])).collect();
        return branch(ctx, &rest, labels, ind, leaf);
    }
    // Two keys that decide together (a base's place: the start drawn and the layout drawn): split on the first, and each
    // group is decided by the second. The pair whose combinations the samples cover best.
    let ks = keys(ctx, items, prefer);
    let mut pair: Option<((i64, i64, usize, usize), Key)> = None;
    for (i, k1) in ks.iter().enumerate() {
        'k2: for (j, k2) in ks.iter().enumerate().skip(i + 1) {
            let mut of: BTreeMap<(i64, i64), usize> = BTreeMap::new();
            for (n, (s, _)) in items.iter().enumerate() {
                let (Some(a), Some(b)) = (key_val(ctx, k1, *s), key_val(ctx, k2, *s)) else { continue 'k2 };
                match of.get(&(a, b)) {
                    Some(l) if *l != labels[n] => continue 'k2,
                    _ => {
                        of.insert((a, b), labels[n]);
                    }
                }
            }
            let combos = domain(ctx, k1) * domain(ctx, k2);
            let score = ((combos - of.len() as i64).max(0), combos, i, j);
            // Kept: the pair's score (uncovered combinations, combinations, order) and its first key.
            if pair.as_ref().is_none_or(|(b, _)| score < *b) {
                pair = Some((score, k1.clone()));
            }
        }
    }
    if let Some((_, k)) = pair {
        let mut groups: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
        for (i, (s, _)) in items.iter().enumerate() {
            groups.entry(key_val(ctx, &k, *s).expect("every sample")).or_default().push(i);
        }
        if groups.len() >= 2 {
            let kl = key_lua(&k);
            for (i, (v, members)) in groups.iter().enumerate() {
                let _ = writeln!(ctx.out, "{ind}{} {kl} == {v} then", if i == 0 { "if" } else { "elseif" });
                let sub: Vec<(usize, &[Tok])> = members.iter().map(|i| items[*i]).collect();
                let sub_labels: Vec<usize> = members.iter().map(|i| labels[*i]).collect();
                branch(ctx, &sub, &sub_labels, &inner, leaf)?;
            }
            let _ = writeln!(ctx.out, "{ind}else
{ind}  unseen({}, {kl})
{ind}end", quoted(&kl));
            return Ok(());
        }
    }
    // Else the key that splits them best: the fewest kinds left, summed over its groups, of the keys every sample has
    // and that leave fewer kinds in some group.
    let mut best: Option<(usize, Key, BTreeMap<i64, Vec<usize>>)> = None;
    for k in keys(ctx, items, prefer) {
        let mut groups: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
        for (i, (s, _)) in items.iter().enumerate() {
            if let Some(v) = key_val(ctx, &k, *s) {
                groups.entry(v).or_default().push(i);
            }
        }
        if groups.len() < 2 || groups.values().map(Vec::len).sum::<usize>() != items.len() {
            continue;
        }
        let per: Vec<usize> = groups.values().map(|g| g.iter().map(|i| labels[*i]).collect::<BTreeSet<_>>().len()).collect();
        let left: usize = per.iter().sum();
        // A draw first when one helps at all: a base is drawn, and what the lobby changes in it (the resources, the
        // initial vehicles) is then a few calls apart, not a copy of the base for each lobby.
        let score = left + match k { Key::Var(_) => 0, _ => 1_000_000 };
        if per.iter().any(|p| *p < kinds.len()) && best.as_ref().is_none_or(|(b, _, _)| score < *b) {
            best = Some((score, k, groups));
        }
    }
    let Some((_, k, groups)) = best else {
        // Where two of them part, for the note.
        let other = labels.iter().position(|l| *l != labels[0]).unwrap_or(0);
        let (a, b) = (items[0].1, items[other].1);
        let at = a.iter().zip(b).position(|(x, y)| shape(x) != shape(y)).unwrap_or(a.len().min(b.len()));
        let show = |t: &[Tok]| t.get(at).map_or("nothing".to_string(), |t| format!("{:?}", shape(t)));
        if std::env::var("OP2_REPLAY_DEBUG").is_ok() {
            for (s, t) in items.iter().take(4) {
                let ks: Vec<String> = keys(ctx, items, prefer).iter().map(|k| format!("{}={:?}", key_lua(k), key_val(ctx, k, *s))).collect();
                eprintln!("{ind}sample {s} (game {}, iter {:?}): {}", ctx.origin[*s], ctx.iter[*s], ks.join(" "));
                let line: Vec<String> = t.iter().take(12).map(|t| format!("{:?}", shape(t))).collect();
                eprintln!("{ind}  {}", line.join(" | "));
            }
        }
        return Err(format!(
            "{} kinds of what follows ({} or {} after {at} alike; {} samples, {} keys), and no draw or lobby input decides which",
            kinds.len(),
            show(a),
            show(b),
            items.len(),
            keys(ctx, items, prefer).len()
        ));
    };
    let kl = key_lua(&k);
    for (i, (v, members)) in groups.iter().enumerate() {
        let _ = writeln!(ctx.out, "{ind}{} {kl} == {v} then", if i == 0 { "if" } else { "elseif" });
        let sub: Vec<(usize, &[Tok])> = members.iter().map(|i| items[*i]).collect();
        let sub_labels: Vec<usize> = members.iter().map(|i| labels[*i]).collect();
        branch(ctx, &sub, &sub_labels, &inner, leaf)?;
    }
    let _ = writeln!(ctx.out, "{ind}else
{ind}  unseen({}, {kl})
{ind}end", quoted(&kl));
    Ok(())
}

/// Labels for `items`: the same for the same `key`.
fn labels_by<K: PartialEq>(items: &[(usize, &[Tok])], key: impl Fn(&[Tok]) -> K) -> Vec<usize> {
    let mut seen: Vec<K> = Vec::new();
    items
        .iter()
        .map(|(_, t)| {
            let k = key(t);
            match seen.iter().position(|x| *x == k) {
                Some(i) => i,
                None => {
                    seen.push(k);
                    seen.len() - 1
                }
            }
        })
        .collect()
}

/// A token's shape, but a call's without a unit's type, as text to compare.
fn loose(t: &Tok) -> String {
    match t {
        Tok::Act { api, kinds, .. } => format!("{api}{kinds:?}"),
        t => format!("{:?}", shape(t)),
    }
}

/// `gaps` alike but for the units' types (an initial vehicle's type by its place in the loop and the colony), written as
/// one call each with every field from a key; false (and nothing written) when they aren't or a field follows no key.
fn merge(ctx: &mut Ctx, gaps: &[(usize, &[Tok])], ind: &str) -> bool {
    let n0 = gaps[0].1.len();
    let calls = |t: &[Tok]| t.iter().all(|t| matches!(t, Tok::Act { .. }));
    if n0 == 0 || !gaps.iter().all(|(_, t)| t.len() == n0 && calls(t) && t.iter().map(loose).eq(gaps[0].1.iter().map(loose))) {
        return false;
    }
    let mark = ctx.out.len();
    for i in 0..n0 {
        let col: Vec<(usize, &[Tok])> = gaps.iter().map(|(s, t)| (*s, &t[i..=i])).collect();
        if let Err(why) = token_alike(ctx, &col, ind) {
            if std::env::var("OP2_REPLAY_DEBUG").is_ok() {
                eprintln!("{ind}merge of {} samples failed: {why}", col.len());
            }
            ctx.out.truncate(mark);
            return false;
        }
    }
    true
}

/// A stretch where the samples may make different things: alike, or an `if` on what decides it. The tokens all share
/// at its start and end go before and after; the rest is merged (`merge`), split on what decides the calls without the
/// units' types, split where the samples first part, or left to `branch`.
fn gap(ctx: &mut Ctx, gaps: &[(usize, &[Tok])], ind: &str) -> Result<(), String> {
    if gaps.iter().all(|(_, t)| t.is_empty()) {
        return Ok(());
    }
    // What every sample makes first and last, alike in shape: before and after the `if` (a draw that decides the rest
    // is made before it). A last token that is a draw stays in the gap.
    let mut gaps: Vec<(usize, &[Tok])> = gaps.to_vec();
    let mut tail: Vec<Vec<(usize, &[Tok])>> = Vec::new();
    loop {
        if gaps.iter().any(|(_, t)| t.is_empty()) {
            break;
        }
        let s0 = shape(&gaps[0].1[0]);
        if gaps.iter().all(|(_, t)| shape(&t[0]) == s0) {
            let col: Vec<(usize, &[Tok])> = gaps.iter().map(|(s, t)| (*s, &t[..1])).collect();
            token(ctx, &col, ind)?;
            gaps = gaps.iter().map(|(s, t)| (*s, &t[1..])).collect();
            continue;
        }
        let e0 = shape(gaps[0].1.last().expect("not empty"));
        if gaps.iter().all(|(_, t)| shape(t.last().expect("not empty")) == e0) && !matches!(e0, Shape::Draws(_)) {
            tail.push(gaps.iter().map(|(s, t)| (*s, &t[t.len() - 1..])).collect());
            gaps = gaps.iter().map(|(s, t)| (*s, &t[..t.len() - 1])).collect();
            continue;
        }
        break;
    }
    if merge(ctx, &gaps, ind) {
        for col in tail.iter().rev() {
            token(ctx, col, ind)?;
        }
        return Ok(());
    }
    // What the samples make, but for the units' types (a truck with its cargo, a tank without): when fewer kinds than
    // with the types and a key decides them, an `if` on it, and each group's units one call with the type from a key.
    let loose_labels = labels_by(&gaps, |t| t.iter().map(loose).collect::<Vec<_>>());
    let exact_kinds = labels_by(&gaps, |t| t.iter().map(shape).collect::<Vec<_>>()).into_iter().collect::<BTreeSet<_>>().len();
    let loose_kinds = loose_labels.iter().collect::<BTreeSet<_>>().len();
    if loose_kinds >= 2 && loose_kinds < exact_kinds
        && let Some((k, by)) = decider(ctx, &gaps, &loose_labels, one_player(&gaps))
    {
        let kl = key_lua(&k);
        let inner = format!("{ind}  ");
        for (i, (l, vs)) in by.iter().enumerate() {
            let cond: Vec<String> = vs.iter().map(|v| format!("{kl} == {v}")).collect();
            let _ = writeln!(ctx.out, "{ind}{} {} then", if i == 0 { "if" } else { "elseif" }, cond.join(" or "));
            let sub: Vec<(usize, &[Tok])> = gaps.iter().zip(&loose_labels).filter(|(_, x)| *x == l).map(|(g, _)| *g).collect();
            if !merge(ctx, &sub, &inner) {
                emit(ctx, &sub, &inner)?;
            }
        }
        let _ = writeln!(ctx.out, "{ind}else
{ind}  unseen({}, {kl})
{ind}end", quoted(&kl));
        for col in tail.iter().rev() {
            token(ctx, col, ind)?;
        }
        return Ok(());
    }
    if !gaps.iter().all(|(_, t)| t.is_empty()) {
        // Where they first part: the key that decides what each makes there decides the branch, and each branch is
        // aligned on its own from there (a base drawn, then what the lobby changes in it).
        let first = labels_by(&gaps, |t| t.first().map(shape));
        let prefer = one_player(&gaps);
        if let Some((k, by)) = decider(ctx, &gaps, &first, prefer) {
            let kl = key_lua(&k);
            let inner = format!("{ind}  ");
            for (i, (l, vs)) in by.iter().enumerate() {
                let cond: Vec<String> = vs.iter().map(|v| format!("{kl} == {v}")).collect();
                let _ = writeln!(ctx.out, "{ind}{} {} then", if i == 0 { "if" } else { "elseif" }, cond.join(" or "));
                let sub: Vec<(usize, &[Tok])> = gaps.iter().zip(&first).filter(|(_, x)| *x == l).map(|(g, _)| *g).collect();
                emit(ctx, &sub, &inner)?;
            }
            let _ = writeln!(ctx.out, "{ind}else
{ind}  unseen({}, {kl})
{ind}end", quoted(&kl));
        } else {
            let labels = labels_by(&gaps, |t| t.iter().map(shape).collect::<Vec<_>>());
            branch(ctx, &gaps, &labels, ind, &|c, i, ind| emit(c, i, ind))?;
        }
    }
    for col in tail.iter().rev() {
        token(ctx, col, ind)?;
    }
    Ok(())
}

/// One token, alike in shape for every sample of `col`; a call whose fields no key gives is made in an `if` on what
/// decides them (a draw or loop that fails is an error).
fn token(ctx: &mut Ctx, col: &[(usize, &[Tok])], ind: &str) -> Result<(), String> {
    match token_alike(ctx, col, ind) {
        Ok(()) => Ok(()),
        Err(why) => {
            if !matches!(col[0].1[0], Tok::Act { .. }) {
                return Err(why);
            }
            let labels = labels_by(col, |t| match &t[0] { Tok::Act { vals, .. } => vals.clone(), _ => Vec::new() });
            branch(ctx, col, &labels, ind, &|c, i, ind| token_alike(c, i, ind))
        }
    }
}

/// One token, alike in shape for every sample of `col`, its fields a constant or what a key gives. A loop becomes a
/// `for` over its iterations; draws become plain draws, a `draw_avoid`, or a `pick` and up to two plain draws after it.
fn token_alike(ctx: &mut Ctx, col: &[(usize, &[Tok])], ind: &str) -> Result<(), String> {
    match &col[0].1[0] {
        Tok::End => Ok(()),
        Tok::Loop(_) => {
            // Each iteration of each game is a sample of the block, with its game's draws and lobby.
            let mut items: Vec<(usize, &[Tok])> = Vec::new();
            for (s, t) in col {
                let Tok::Loop(iters) = &t[0] else { unreachable!() };
                let game = ctx.origin[*s];
                if iters.len() as i64 != ctx.samples[game].env.units {
                    return Err("a block repeated other than once per initial vehicle".into());
                }
                for (j, it) in iters.iter().enumerate() {
                    ctx.origin.push(game);
                    ctx.iter.push(Some(j as i64));
                    items.push((ctx.origin.len() - 1, &it[..]));
                }
            }
            // The loop's draws take roles of their own, keyed by this loop.
            let outer = ctx.vars.scope;
            ctx.vars.scope = ctx.origin.len();
            let _ = writeln!(ctx.out, "{ind}for i = 1, game.initial_units() do");
            let r = emit(ctx, &items, &format!("{ind}  "));
            let _ = writeln!(ctx.out, "{ind}end");
            ctx.vars.scope = outer;
            r
        }
        Tok::Draws { range, .. } => {
            let range = *range;
            let runs: Vec<(usize, &Vec<i64>)> = col.iter().map(|(s, t)| match &t[0] { Tok::Draws { vals, .. } => (*s, vals), _ => unreachable!() }).collect();
            // A fixed number of draws, or a pick of k different values (redrawing one already drawn).
            let fixed = runs.iter().all(|(_, v)| v.len() == runs[0].1.len());
            let distinct = |v: &Vec<i64>| -> Option<Vec<i64>> {
                let mut got: Vec<i64> = Vec::new();
                for x in v {
                    if !got.contains(x) {
                        got.push(*x);
                    }
                }
                // a pick ends on a new value
                (v.last().is_some_and(|l| got.last() == Some(l))).then_some(got)
            };
            let picked: Option<Vec<(usize, Vec<i64>)>> = runs.iter().map(|(s, v)| distinct(v).map(|g| (*s, g))).collect();
            // The same number of draws in every sample: draws of their own (a value may come up twice). A number that
            // varies is a `draw_avoid` or a pick, below.
            if fixed {
                for j in 0..runs[0].1.len() {
                    let i = new_var(&mut ctx.vars, range, runs.iter().map(|(s, v)| (*s, v[j])).collect());
                    let _ = writeln!(ctx.out, "{ind}R[{i}] = game.rand({range})");
                }
                return Ok(());
            }
            // One draw, redrawn while it's one of some earlier draws of this range (each beacon of a group on a spot of
            // its own; Wishbone's draws 1, 3, 5 apart from 2, 4, 6): of the earlier draws, those no game's kept value
            // equals, and of them the fewest, latest first, that every game's redraws come from.
            // A spot taken may also be an earlier draw plus a fixed offset (Flood Plain: a common beacon on the spot drawn
            // and a rare one 11 spots on, of 22). The candidates are each earlier draw of this range every sample has,
            // with each offset under the range (64 at most), latest draw first.
            let at = |(i, c): (usize, i64), s: usize| var_val(ctx, i, s).map(|v| (v + c).rem_euclid(range));
            let prev: Vec<(usize, i64)> = (0..ctx.vars.vals.len())
                .rev()
                .filter(|i| ctx.vars.range[*i] == range && runs.iter().all(|(s, _)| var_val(ctx, *i, *s).is_some()))
                .flat_map(|i| (0..range.min(64)).map(move |c| (i, c)))
                .filter(|t| runs.iter().all(|(s, v)| at(*t, *s) != v.last().copied()))
                .collect();
            let covers = |set: &[(usize, i64)]| {
                runs.iter().all(|(s, v)| {
                    let taken: BTreeSet<i64> = set.iter().filter_map(|t| at(*t, *s)).collect();
                    v[..v.len() - 1].iter().all(|x| taken.contains(x))
                })
            };
            let avoid = covers(&prev).then(|| {
                // the fewest: each candidate left out where the rest still cover every redraw, those with an offset
                // tried first, then the plain draws, oldest first in each
                let mut set = prev.clone();
                let order: Vec<(usize, i64)> = prev.iter().rev().filter(|t| t.1 != 0).chain(prev.iter().rev().filter(|t| t.1 == 0)).copied().collect();
                for j in order {
                    let without: Vec<(usize, i64)> = set.iter().copied().filter(|x| *x != j).collect();
                    if covers(&without) {
                        set = without;
                    }
                }
                set
            });
            if let Some(set) = avoid.filter(|set| !set.is_empty()) {
                let i = new_var(&mut ctx.vars, range, runs.iter().map(|(s, v)| (*s, *v.last().expect("a draw"))).collect());
                ctx.avoids = true;
                let taken: Vec<String> =
                    set.iter().map(|(j, c)| if *c == 0 { format!("R[{j}]") } else { format!("(R[{j}] + {c}) % {range}") }).collect();
                let _ = writeln!(ctx.out, "{ind}R[{i}] = draw_avoid({range}, {{ {} }})", taken.join(", "));
                return Ok(());
            }
            // A pick, or a pick then a plain draw or two of the same range (Axen's Home's Land Rush `mu6_01`: a start for each
            // player, then one more draw): the first whose number of picks follows the lobby or a draw. The pick's
            // values get an `R` each (a sample that picked fewer has none for the last ones).
            let mut why = format!("draws of {range} that aren't a pick of different values");
            for m in 0..=2usize {
                let picked: Option<Vec<(usize, Vec<i64>)>> = if m == 0 {
                    picked.clone()
                } else {
                    runs.iter().map(|(s, v)| if v.len() > m { distinct(&v[..v.len() - m].to_vec()).map(|g| (*s, g)) } else { None }).collect()
                };
                let Some(picked) = picked else { continue };
                let k = match expr(ctx, col, None, &|s| Val::Int(picked.iter().find(|(x, _)| *x == s).map_or(0, |(_, g)| g.len() as i64))) {
                    Ok(k) => k,
                    Err(w) => {
                        if m == 0 {
                            why = format!("a pick of {range}: {w}");
                        }
                        continue;
                    }
                };
                let kmax = picked.iter().map(|(_, g)| g.len()).max().unwrap_or(0);
                ctx.picks = true;
                let mut at = Vec::new();
                for j in 0..kmax {
                    at.push(new_var(&mut ctx.vars, range, picked.iter().filter_map(|(s, g)| g.get(j).map(|v| (*s, *v))).collect()).to_string());
                }
                let _ = writeln!(ctx.out, "{ind}pick({range}, {k}, {{ {} }})", at.join(", "));
                for j in 0..m {
                    let i = new_var(&mut ctx.vars, range, runs.iter().map(|(s, v)| (*s, v[v.len() - m + j])).collect());
                    let _ = writeln!(ctx.out, "{ind}R[{i}] = game.rand({range})");
                }
                return Ok(());
            }
            Err(why)
        }
        Tok::Act { api, kinds, .. } => {
            let api = *api;
            let n = kinds.len();
            // The player this call is about, when it's the same in every sample.
            let prefer = kinds.iter().position(|k| *k == Kind::Player).and_then(|f| {
                let ps: BTreeSet<i64> = col.iter().filter_map(|(_, t)| match &t[0] { Tok::Act { vals, .. } => if let Val::Int(p) = vals[f] { Some(p) } else { None }, _ => None }).collect();
                (ps.len() == 1).then(|| *ps.iter().next().expect("one") as usize)
            });
            let mut fields = Vec::new();
            for f in 0..n {
                let get = |s: usize| -> Val {
                    let t = col.iter().find(|(x, _)| *x == s).map(|(_, t)| &t[0]);
                    match t { Some(Tok::Act { vals, .. }) => vals[f].clone(), _ => Val::Int(0) }
                };
                fields.push(expr(ctx, col, prefer, &get).map_err(|why| format!("{api} field {f}: {why}"))?);
            }
            let _ = writeln!(ctx.out, "{ind}{}", act_lua(api, &fields));
            Ok(())
        }
    }
}

/// A lobby of `n` players: the `i`-th; the first ones the harness's start (high resources, no initial vehicles, the
/// host Eden, odd slots Eden), the others drawn by `next`. op2-missionhost gives every player the host's resource level,
/// so a lobby has one.
fn lobby(n: u8, i: u32, next: &mut dyn FnMut(u64) -> u64) -> NetStart {
    let plain = i < PLAIN;
    let level = if plain { 0 } else { next(3) as i32 };
    NetStart {
        seed: crate::luaexport::seed(i * 7 + u32::from(n)),
        slots: (0..i32::from(n))
            .map(|p| NetSlot { eden: if plain { p == 0 || p & 1 == 1 } else { next(2) == 1 }, color: (p + 1) % 6, resources: level })
            .collect(),
        day_night: plain || next(2) == 1,
        disasters: !plain && next(2) == 1,
        morale_steady: plain || next(2) == 1,
        initial_vehicles: if plain { 0 } else { [0, 0, 4, 8, 12][next(5) as usize] },
        lift_limits: false,
        classic_rules: true,
    }
}

/// Games of the harness's start first, at each player count.
const PLAIN: u32 = 8;
/// At each player count: at least this many games, and more until `QUIET` in a row bring nothing new, up to `MOST`.
const LEAST: u32 = 48;
/// Games in a row that show nothing new (`seen`) before the sampling of a player count stops.
const QUIET: u32 = 48;
/// The most games at each player count.
const MOST: u32 = 1500;
/// The longest replay kept, in bytes (OP2_REPLAY_MAX_KB overrides it).
const MOST_LUA: usize = 8 << 20;

/// What a game shows of how its draws go: each draw's value by where it comes (the k-th draw), each pair of them, and
/// each with the lobby's choices (a player's colony, the resource level, the initial vehicles). Each entry is (k, its
/// value, what it's paired with: -1 nothing, j the j-th draw, 1000 + p player p's colony, 2000 the level, 3000 the
/// initial vehicles, and that one's value). The first 64 draws count.
fn seen(s: &Sample) -> Vec<(i64, i64, i64, i64)> {
    // A pick's values as picked (its redraws left out), so the k-th draw is where it comes in every game.
    let draws: Vec<i64> = s
        .toks
        .iter()
        .flat_map(|t| match t {
            Tok::Draws { vals, .. } => {
                let mut got: Vec<i64> = Vec::new();
                for v in vals {
                    if !got.contains(v) {
                        got.push(*v);
                    }
                }
                if got.len() < vals.len() { got } else { vals.clone() }
            }
            _ => Vec::new(),
        })
        .collect();
    let mut out = Vec::new();
    // The values of draws of a wide range (over 32; a Land Rush start's x and y) don't count (-1): they come up new for
    // ever, and a start's tile follows its draw by arithmetic, not a table. `ranges` lines up with `draws`.
    let ranges: Vec<i64> = s
        .toks
        .iter()
        .flat_map(|t| match t {
            Tok::Draws { range, vals } => {
                let distinct = vals.iter().collect::<BTreeSet<_>>().len();
                vec![*range; if distinct < vals.len() { distinct } else { vals.len() }]
            }
            _ => Vec::new(),
        })
        .collect();
    let draws: Vec<i64> = draws.iter().zip(&ranges).map(|(v, r)| if *r > 32 { -1 } else { *v }).collect();
    let d = draws.len().min(64);
    for i in 0..d {
        let i64i = i as i64;
        out.push((i64i, draws[i], -1, 0));
        for j in i + 1..d {
            out.push((i64i, draws[i], j as i64, draws[j]));
        }
        for (p, e) in s.env.eden.iter().enumerate() {
            out.push((i64i, draws[i], 1000 + p as i64, *e));
        }
        // Every player has the host's level, so the host's stands for all.
        out.push((i64i, draws[i], 2000, s.env.level.first().copied().unwrap_or(0)));
        out.push((i64i, draws[i], 3000, s.env.units));
    }
    out
}

/// The replay of `dll`'s setup (2 to `max` players, on a map padded by `pad`): the helpers it needs before `on_init`
/// (`R`, `U`, `unseen` and, when used, `draw_avoid` and `pick`), and the Lua of `on_init` up to the setup's first trigger.
/// An error when a run can't be tokenized or no way of writing it replays every sample. With a computer player (`ai`),
/// its setup is a `setup_ai(players[n + 1])` call where the original makes it, and `n` counts the humans.
pub fn convert(dll: &Path, max: u8, pad: i32, ai: bool, progress: &mut dyn FnMut(&str)) -> Result<(String, String)> {
    let mut samples = Vec::new();
    // A xorshift with a fixed seed: the lobbies drawn are the same on every run.
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = |m: u64| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x % m
    };
    for n in 2..=max {
        let mut known: BTreeSet<(i64, i64, i64, i64)> = BTreeSet::new();
        let (mut i, mut quiet) = (0u32, 0u32);
        while i < MOST && (i < LEAST || quiet < QUIET) {
            let lobby = lobby(n, i, &mut next);
            i += 1;
            let env = Env {
                n: lobby.slots.len() as i64,
                eden: lobby.slots.iter().map(|s| i64::from(s.eden)).collect(),
                level: lobby.slots.iter().map(|s| i64::from(s.resources)).collect(),
                units: i64::from(lobby.initial_vehicles),
                day_night: i64::from(lobby.day_night),
                morale_free: i64::from(!lobby.morale_steady),
                disasters: i64::from(lobby.disasters),
            };
            let opts = lobby.options(Options { call_log: true, ..Options::from_env() });
            let calls = crate::calls::record_with(dll, &opts, 0, &[])?;
            let toks = tokens(&calls, &env, i64::from(pad), ai).map_err(|why| anyhow::anyhow!("the setup isn't replayed: {why}"))?;
            let looped = loops(toks.clone(), env.units.max(0) as usize);
            let sample = Sample { env, toks, looped };
            let mut new = false;
            for k in seen(&sample) {
                new |= known.insert(k);
            }
            quiet = if new { 0 } else { quiet + 1 };
            samples.push(sample);
        }
        progress(&format!("replay: {n} players, {i} games"));
    }
    progress(&format!("replay: {} games", samples.len()));

    // Two replays: every player count together, and each count on its own (the same draw can mean other things with
    // more players: a start drawn for each, or one drawn and the others following from it). The shorter one that
    // works is kept: a replay that follows draws telling the samples apart by chance comes out long.
    let counts: BTreeSet<i64> = samples.iter().map(|s| s.env.n).collect();
    // Each game's draws one by one, not in runs of a range (Fractures: a draw of 100 for each of an area's beacon spots,
    // a beacon where it comes under 45), for when the runs don't replay.
    let one_by_one: Vec<(Vec<Tok>, Vec<Tok>)> = samples
        .iter()
        .map(|s| {
            let toks = singles(&s.toks);
            let looped = loops(toks.clone(), s.env.units.max(0) as usize);
            (toks, looped)
        })
        .collect();
    // One way of writing the replay: every player count together or an `if` on `n`, the initial vehicles as a loop or
    // written out, the draws in runs or one by one. An error when a sample can't be replayed or the Lua is too long.
    let attempt = |by_count: bool, looped: bool, single: bool| -> Result<Ctx, String> {
        let items: Vec<(usize, &[Tok])> = samples
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let t: &[Tok] = match (single, looped) {
                    (false, true) => &s.looped,
                    (false, false) => &s.toks,
                    (true, true) => &one_by_one[i].1,
                    (true, false) => &one_by_one[i].0,
                };
                (i, t)
            })
            .collect();
        let mut ctx = Ctx {
            samples: &samples,
            vars: Vars { vals: Vec::new(), range: Vec::new(), count: BTreeMap::new(), role: BTreeMap::new(), scope: 0 },
            out: String::new(),
            picks: false,
            avoids: false,
            origin: (0..samples.len()).collect(),
            iter: vec![None; samples.len()],
        };
        if !by_count {
            emit(&mut ctx, &items, "  ")?;
        } else {
            for (i, n) in counts.iter().enumerate() {
                let _ = writeln!(ctx.out, "  {} n == {n} then", if i == 0 { "if" } else { "elseif" });
                let sub: Vec<(usize, &[Tok])> = items.iter().filter(|(s, _)| samples[*s].env.n == *n).copied().collect();
                emit(&mut ctx, &sub, "    ").map_err(|why| format!("with {n} players: {why}"))?;
            }
            let _ = writeln!(ctx.out, "  end");
        }
        let most = std::env::var("OP2_REPLAY_MAX_KB").ok().and_then(|v| v.parse::<usize>().ok()).map_or(MOST_LUA, |kb| kb << 10);
        if ctx.out.len() > most {
            return Err(format!("its replay came out {} KB (what decides its choices wasn't found)", ctx.out.len() / 1024));
        }
        Ok(ctx)
    };
    // Every way with the draws in runs, then (only when none of those worked) with them one by one; by player count
    // only when there are two counts or more. The shortest that works is kept, and the first error is reported.
    let mut best: Option<Ctx> = None;
    let mut why = String::new();
    let ways = [(false, true), (false, false), (true, true), (true, false)];
    for (single, (by_count, looped)) in ways.iter().map(|w| (false, *w)).chain(ways.iter().map(|w| (true, *w))) {
        if (by_count && counts.len() < 2) || (single && best.is_some()) {
            continue;
        }
        let got = attempt(by_count, looped, single);
        if std::env::var("OP2_REPLAY_DEBUG").is_ok() {
            eprintln!(
                "attempt by_count={by_count} looped={looped} single={single}: {}",
                match &got { Ok(c) => format!("{} KB", c.out.len() / 1024), Err(w) => w.clone() }
            );
        }
        match got {
            Ok(c) => {
                if best.as_ref().is_none_or(|b| c.out.len() < b.out.len()) {
                    best = Some(c);
                }
            }
            Err(w) => {
                if why.is_empty() {
                    why = w;
                }
            }
        }
    }
    let Some(ctx) = best else { anyhow::bail!("the setup isn't replayed: {why}") };
    let mut head = String::new();
    head.push_str("-- The setup's draws (R) and the units it made (U), in order.\nlocal R, U = {}, {}\n");
    head.push_str("local LEVEL_OF = { Easy = 0, Normal = 1, Hard = 2 }\n");
    let _ = writeln!(head, "local DIRS = {{ {} }}", DIRECTIONS.iter().map(|d| quoted(d)).collect::<Vec<_>>().join(", "));
    head.push_str(
        "-- A branch of the setup no sampled game took: logged, so a game that differs from the original says where.
         local function unseen(what, value)
  print(\"replay: \" .. what .. \" = \" .. tostring(value) .. \": a way no sampled game took\")
end
",
    );
    if ctx.avoids {
        head.push_str(
            "-- A value of 0 to range - 1, redrawn while it's one of `taken`.
             local function draw_avoid(range, taken)
  local no, left = {}, range
  for _, t in ipairs(taken) do
    if t >= 0 and t < range and not no[t] then no[t] = true; left = left - 1 end
  end
  if left == 0 then
    print(\"replay: draw_avoid(\" .. range .. \"): every value taken, a way no sampled game took\")
    return game.rand(range)
  end
  local v
  repeat v = game.rand(range) until not no[v]
  return v
end
",
        );
    }
    if ctx.picks {
        head.push_str(
            "-- k different values of 0 to range - 1, each redrawn while it's one already drawn, into R[at[1]] on.\n\
             local function pick(range, k, at)\n  local seen = {}\n  for i = 1, k do\n    local v\n    \
             repeat v = game.rand(range) until not seen[v]\n    seen[v] = true\n    R[at[i]] = v\n  end\nend\n",
        );
    }
    Ok((head, ctx.out))
}
