//! The computer player of a converted mission (`op2dll lua`): the game's own AI groups, set up as the mission's setup
//! sets them up, written with OP2Lua's `ai.*` calls.
//!
//! A stock map with a computer player (Peacekeeper, La Corrida; `DescBlockEx` asks for one) gives it a base the game
//! keeps standing: a building group that rebuilds what it has recorded and makes the vehicles the other groups want,
//! mining groups whose trucks run ore, and fight groups that guard. Its setup calls on the computer player, its groups
//! and units are written out in order, as calls of the same name. For a multiplayer map that is one setup with the
//! most players, the computer player the one after the humans; a colony game or campaign mission (colony.rs) names
//! its player, converts Normal's setup and merges the other difficulties' in (`add_level`). Unit tables (`UnitBlock`)
//! are read from the DLL as it ships, before its `DllMain` sorts them, so OP2Lua's `ai.unit_block` hands the game the
//! same table to sort.
//!
//! Trigger functions that only work on the groups (an "Expand" that records more of the base, a timed upgrade of the
//! guards' weapons) are written the same way from repeated calls: as one body when every call does the same, or as
//! steps, one per call, when the calls go through a list and then repeat one ending.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use op2_missionhost::runtime::{CallRec, Mission, Options};

use crate::luaexport::{lua_name, quoted};

/// How many calls of a trigger function are read to find its steps.
pub const RUNS: usize = 40;

/// OP2Lua's direction names, in the game's order: a `CreateUnit` rotation 0-7, or a unit record's rotation / 32.
const DIRECTIONS: [&str; 8] = ["East", "SouthEast", "South", "SouthWest", "West", "NorthWest", "North", "NorthEast"];

/// The computer player as the setup made it, and the names its Lua uses.
pub struct Ai {
    /// The computer player's number, 0-based.
    pub player: i32,
    /// The x offset of the game's tiles from the map file's (32, or 0 on a 512-wide map).
    pad: i32,
    /// The map's width in tiles: a tile beyond it is passed to the game as it is (`raw`).
    width: i32,
    /// The setup's groups: stub id to Lua name.
    groups: BTreeMap<i64, String>,
    /// Each UnitBlock's records (by the block's address), as the DLL ships them.
    blocks: BTreeMap<u32, Vec<[i32; 8]>>,
    /// The DLL's exported unit blocks `CreateUnitBlock` names: name to the block's address (a key of `blocks`).
    named: BTreeMap<String, u32>,
    /// The blocks used (addresses), in order of first use: a block's place here + 1 is its index in `AI_UNITS`.
    used: Vec<u32>,
    /// The setup's triggers: stub id to the function each calls.
    triggers: BTreeMap<i64, String>,
    /// Units the setup made: id to (owner, type code).
    units: BTreeMap<i64, (i32, String)>,
    /// And their tiles (file tiles).
    unit_tiles: BTreeMap<i64, (i32, i32)>,
    /// Whether the Lua finds a unit by its tile (`unit_at`), for a call that names one the setup made outside a group.
    unit_at: bool,
    /// `setup_ai`'s body.
    setup: String,
    /// Top-level locals the trigger functions keep (a step counter).
    counters: Vec<String>,
    /// The groups only another difficulty's setup makes (`add_level`).
    level_groups: Vec<String>,
    /// The DLL as it ships, for the patrol routes its groups are given.
    image: Mission,
}

/// The arguments of a call, split at top-level commas (a location's or rectangle's own commas kept).
fn args(c: &CallRec) -> Vec<String> {
    let all = c.args.join(", ");
    let mut out = Vec::new();
    let (mut depth, mut cur) = (0, String::new());
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

/// The integers in `s`, in order (a `-` counts only at a number's start).
fn ints(s: &str) -> Vec<i64> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_digit() || (ch == '-' && cur.is_empty()) {
            cur.push(ch);
        } else {
            if let Ok(v) = cur.parse() {
                out.push(v);
            }
            cur.clear();
        }
    }
    out
}

/// The stub or unit number of `Kind#n`.
fn hash_id(s: &str) -> Option<i64> {
    s.rsplit('#').next()?.trim().parse().ok()
}

/// The hex address after the last `0x` in `s` (`UnitBlock@0x...`).
fn address(s: &str) -> Option<u32> {
    u32::from_str_radix(s.rsplit("0x").next()?.trim(), 16).ok()
}

/// The game's map id of a type code (`LYNX`), 0 for an unknown one.
fn map_id(code: &str) -> i32 {
    op2_missionhost::names::MAP_IDS.iter().position(|c| *c == code).map_or(0, |i| i as i32)
}

/// A type code as OP2Lua names it, quoted, or `nil` for none.
fn type_arg(code: &str) -> String {
    if code == "None" || code.is_empty() { "nil".into() } else { quoted(&lua_name(map_id(code))) }
}

/// A record table of the DLL as it ships: 32-byte records up to one of no type (at most 4096).
fn records(m: &Mission, mut a: u32) -> Vec<[i32; 8]> {
    let mut out = Vec::new();
    while out.len() < 4096 {
        let r: [i32; 8] = std::array::from_fn(|i| m.m.read_u32(a + 4 * i as u32) as i32);
        if r[0] == 0 {
            break;
        }
        out.push(r);
        a += 32;
    }
    out
}

impl Ai {
    /// A tile `(x, y)` in the game's coordinates as a Lua tile (status-bar numbers); one off the map as the game's
    /// own tile with `raw = true`.
    fn at(&self, s: &str) -> Result<String, String> {
        match ints(s)[..] {
            // Off the map (CPS2's truck at x 1262): the game's own tile, which the game puts at the edge.
            [x, y] if !(0..i64::from(self.width)).contains(&(x - i64::from(self.pad))) => Ok(format!("{{ x = {x}, y = {y}, raw = true }}")),
            [x, y] => Ok(format!("{{ {}, {} }}", x - self.pad as i64 + 1, y + 1)),
            _ => Err(format!("a location {s}")),
        }
    }

    /// A rectangle `x1, y1, x2, y2` in the game's coordinates as a Lua rectangle (status-bar numbers).
    fn rect(&self, s: &str) -> Result<String, String> {
        match ints(s)[..] {
            [x1, y1, x2, y2] => {
                // Less the padding, 1-based: x - pad + 1.
                let p = self.pad as i64 - 1;
                Ok(format!("{{ {}, {}, {}, {} }}", x1 - p, y1 + 1, x2 - p, y2 + 1))
            }
            _ => Err(format!("a rectangle {s}")),
        }
    }

    /// The UnitBlock at the address in `s` as `ai_block(i)`, numbered on first use; an error for a block the DLL's
    /// start didn't build.
    fn block(&mut self, s: &str) -> Result<String, String> {
        let a = address(s).ok_or_else(|| format!("a unit block {s}"))?;
        if !self.blocks.contains_key(&a) {
            return Err(format!("a unit block the DLL's start didn't make ({s})"));
        }
        let i = match self.used.iter().position(|u| *u == a) {
            Some(i) => i,
            None => {
                self.used.push(a);
                self.used.len() - 1
            }
        };
        Ok(format!("ai_block({})", i + 1))
    }

    /// `setup_ai`'s body.
    pub fn setup_body(&self) -> &str {
        &self.setup
    }

    /// Sets `setup_ai`'s body (the difficulties' setups merged).
    pub fn set_setup_body(&mut self, body: String) {
        self.setup = body;
    }

    /// Another difficulty's setup body (`level`: its name) in this one's terms: its unit blocks numbered among this
    /// one's (`ai_block`), its units and groups named as this one's where they match, and the groups only it makes
    /// named for the difficulty and declared with this one's.
    pub fn add_level(&mut self, other: &Ai, level: &str) -> String {
        // Each `ai_block(k)` renumbered to the same block's number here, a block only `other` uses added.
        let mut body = String::new();
        let mut rest = other.setup.as_str();
        while let Some(i) = rest.find("ai_block(") {
            let (head, tail) = rest.split_at(i + "ai_block(".len());
            body.push_str(head);
            let end = tail.find(')').unwrap_or(0);
            match tail[..end].parse::<usize>().ok().and_then(|k| other.used.get(k.wrapping_sub(1))) {
                Some(a) => {
                    let k = self.used.iter().position(|u| u == a).unwrap_or_else(|| {
                        self.used.push(*a);
                        self.blocks.entry(*a).or_insert_with(|| other.blocks[a].clone());
                        self.used.len() - 1
                    });
                    body.push_str(&(k + 1).to_string());
                    rest = &tail[end..];
                }
                None => rest = tail,
            }
        }
        body.push_str(rest);
        // A unit local (`u61`) renamed to this setup's unit of the same owner, type and tile, when exactly one is.
        let key = |ai: &Ai, id: i64| Some((ai.units.get(&id)?.clone(), *ai.unit_tiles.get(&id)?));
        let body = rename_words(&body, |w| {
            let id: i64 = w.strip_prefix('u')?.parse().ok()?;
            let k = key(other, id)?;
            match self.units.keys().copied().filter(|m| key(self, *m).as_ref() == Some(&k)).collect::<Vec<_>>()[..] {
                [m] => Some(format!("u{m}")),
                _ => None,
            }
        });
        let names = self.group_names(other, &body, level);
        let body = rename_words(&body, |w| names.get(w).cloned());
        for g in names.values() {
            if !self.groups.values().any(|n| n == g) && !self.level_groups.contains(g) {
                self.level_groups.push(g.clone());
            }
        }
        self.unit_at |= other.unit_at;
        body
    }

    /// `other`'s groups by this setup's names: each paired with this one's group of its kind whose setup calls in
    /// `body` (`other`'s setup, its blocks and units already renamed) have most lines alike with it, the best pairs
    /// first; one alike with none (or whose match is taken) keeps its name with `level` added (`ai_fight3_easy`).
    fn group_names(&self, other: &Ai, body: &str, level: &str) -> BTreeMap<String, String> {
        // The lines naming group `g`, besides the one that makes it, with `g` written as `G`.
        let lines_of = |text: &str, g: &str| -> Vec<String> {
            let made = format!("{g} = ");
            text.lines()
                .filter(|l| !l.trim_start().starts_with(&made))
                .filter(|l| !rename_words(l, |w| (w == g).then(String::new)).eq(*l))
                .map(|l| rename_words(l.trim(), |w| (w == g).then(|| "G".to_string())))
                .collect()
        };
        let kind = |g: &str| g.trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
        // How many of `a`'s lines `b` has, each of `b`'s matched once.
        let alike = |a: &[String], b: &[String]| {
            let mut b = b.to_vec();
            a.iter().filter(|l| b.iter().position(|m| m == *l).map(|i| b.remove(i)).is_some()).count()
        };
        let mut pairs = Vec::new();
        for o in other.groups.values() {
            let lo = lines_of(body, o);
            for s in self.groups.values().filter(|s| kind(s) == kind(o)) {
                let n = alike(&lo, &lines_of(&self.setup, s));
                if n > 0 {
                    pairs.push((n, o.clone(), s.clone()));
                }
            }
        }
        pairs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| (&a.1, &a.2).cmp(&(&b.1, &b.2))));
        let mut names = BTreeMap::new();
        let mut taken = BTreeSet::new();
        for (_, o, s) in pairs {
            if !names.contains_key(&o) && taken.insert(s.clone()) {
                names.insert(o, s);
            }
        }
        for o in other.groups.values() {
            names.entry(o.clone()).or_insert_with(|| format!("{o}_{}", level.to_lowercase()));
        }
        names
    }

    /// The number the game gave the first of the setup's groups (triggers and groups share the numbering).
    pub fn first_group(&self) -> Option<i64> {
        self.groups.keys().min().copied()
    }

    /// `AI_UNITS` and `ai_block`, the groups' and counters' locals: the Lua before the functions.
    pub fn header(&self) -> String {
        let mut h = String::new();
        let _ = writeln!(h, "-- The computer player: the game's own AI groups, as the mission sets them up (the game runs them).");
        if !self.used.is_empty() {
            let _ = writeln!(h, "-- Its unit tables, as the mission's UnitRecord arrays list them (ai.unit_block).");
            let _ = writeln!(h, "local AI_UNITS = {{");
            for (i, a) in self.used.iter().enumerate() {
                let _ = writeln!(h, "  [{}] = {{", i + 1);
                for r in &self.blocks[a] {
                    // A record: type, x, y, _, rotation, weapon, classification (16 the default, left out), cargo.
                    let mut f = vec![format!("type = {}", quoted(&lua_name(r[0]))), format!("at = {{ {}, {} }}", r[1] - self.pad + 1, r[2] + 1)];
                    match r[4] {
                        0 => {}
                        rot if rot % 32 == 0 && (0..256).contains(&rot) => f.push(format!("direction = \"{}\"", DIRECTIONS[(rot / 32) as usize])),
                        rot => f.push(format!("rotation = {rot}")),
                    }
                    if r[5] != 0 {
                        f.push(format!("weapon = {}", quoted(&lua_name(r[5]))));
                    }
                    if r[6] != 16 {
                        f.push(format!("classification = {}", r[6]));
                    }
                    let _ = writeln!(h, "    {{ {} }},", f.join(", "));
                }
                let _ = writeln!(h, "  }},");
            }
            let _ = writeln!(h, "}}");
            let _ = writeln!(h, "local ai_blocks = {{}}");
            let _ = writeln!(h, "local function ai_block(i)");
            let _ = writeln!(h, "  ai_blocks[i] = ai_blocks[i] or ai.unit_block(AI_UNITS[i])");
            let _ = writeln!(h, "  return ai_blocks[i]");
            let _ = writeln!(h, "end");
        }
        if self.unit_at {
            let _ = writeln!(h, "-- A unit of a type on a tile (one the setup made, which a group's call names).");
            let _ = writeln!(h, "local function unit_at(kind, at)");
            let _ = writeln!(h, "  for _, u in ipairs(game.units_in_rect({{ at[1], at[2], at[1], at[2] }})) do");
            let _ = writeln!(h, "    if u.type == kind then return u end");
            let _ = writeln!(h, "  end");
            let _ = writeln!(h, "end");
        }
        let mut locals = vec!["ai_player".to_string()];
        locals.extend(self.groups.values().cloned());
        locals.extend(self.level_groups.iter().cloned());
        let _ = writeln!(h, "local {}", locals.join(", "));
        for c in &self.counters {
            let _ = writeln!(h, "local {c} = 0");
        }
        let _ = writeln!(h, "\nlocal function setup_ai(p)");
        let _ = writeln!(h, "  ai_player = p");
        h.push_str(&self.setup);
        let _ = writeln!(h, "end\n");
        h
    }
}

/// The calls of a setup or function that concern the computer player, as Lua lines.
struct Lines<'a> {
    /// The computer player: its groups, blocks and units, which the lines add to.
    ai: &'a mut Ai,
    /// Groups made here: stub id to their local name.
    made: BTreeMap<i64, String>,
    /// Whether made groups become top-level (the setup) or locals (a function).
    top: bool,
    /// The function's own name, when writing a function: a `Destroy` of the trigger that calls it is `t:destroy()`.
    me: Option<&'a str>,
    /// The Lua lines written so far.
    out: Vec<String>,
    /// A `create_unit` not yet taken by a group.
    pending: Option<String>,
    /// The units made here that later calls name: id to the local holding it.
    made_units: BTreeMap<i64, String>,
    /// The units the run's `Unit::` calls name.
    named_later: BTreeSet<i64>,
    /// `walking` comes from a `GetFirstOfType` lookup, not a walk of the group.
    lookup: bool,
    /// The group a `GroupEnumerator` walks.
    walking: Option<String>,
    /// The unit and its Lua name of the latest `first_of_type`, while its calls go on.
    picked: Option<(i64, String)>,
    /// The rectangle and tile of the latest `MAP_RECT::RandPt`, for the `CreateUnit` on that tile.
    rand_pt: Option<(String, String)>,
    /// The functions the triggers made here call.
    calls: BTreeSet<String>,
}

impl Lines<'_> {
    /// The Lua name of the group `s` (`FightGroup#n`) names: one made here, else one of the setup's.
    fn group(&self, s: &str) -> Result<String, String> {
        let id = hash_id(s).ok_or_else(|| format!("a group {s}"))?;
        self.made.get(&id).or_else(|| self.ai.groups.get(&id)).cloned().ok_or_else(|| format!("a group the computer player hasn't ({s})"))
    }

    /// Writes the `create_unit` no group took and closes the `if` of a `first_of_type` pick.
    fn flush(&mut self) {
        // A unit already kept as a local (`{ u61 }`) that no group took: nothing more to write.
        if let Some(u) = self.pending.take().filter(|u| !(u.starts_with("{ u") && u.ends_with(" }"))) {
            self.out.push(u);
        }
        if self.picked.take().is_some() {
            self.out.push("end".into());
        }
    }

    /// Writes `line` after whatever is pending.
    fn push(&mut self, line: String) {
        self.flush();
        self.out.push(line);
    }

    /// One call; calls that don't concern the computer player are left out, and an error says why a call that does
    /// can't be written.
    fn call(&mut self, c: &CallRec) -> Result<(), String> {
        let a = args(c);
        let ai = format!("Player{}", self.ai.player);
        let this = a.first().and_then(|t| t.strip_prefix("this=")).unwrap_or("");
        let api = c.api.as_str();
        let (class, method) = api.split_once("::").unwrap_or(("", api));
        match class {
            "" if method.starts_with("Create") && method.ends_with("Group") => {
                if a.first().map(String::as_str) != Some(ai.as_str()) {
                    return Ok(());
                }
                let id = hash_id(&c.ret).ok_or("a group without a number")?;
                let (kind, stem) = match method {
                    "CreateBuildingGroup" => ("building_group", "base"),
                    "CreateFightGroup" => ("fight_group", "fight"),
                    "CreateMiningGroup" => ("mining_group", "mining"),
                    other => return Err(format!("it calls {other}")),
                };
                // The setup's groups are top-level (`ai_fight1`), a function's are its locals (`fight1`).
                let taken: BTreeSet<&String> = self.ai.groups.values().chain(self.made.values()).collect();
                let name = (1..).map(|n| if self.top { format!("ai_{stem}{n}") } else { format!("{stem}{n}") }).find(|n| !taken.contains(n)).expect("a name");
                let line = if self.top { format!("{name} = ai.{kind}(p)") } else { format!("local {name} = ai.{kind}(ai_player)") };
                if self.top {
                    self.ai.groups.insert(id, name.clone());
                } else {
                    self.made.insert(id, name.clone());
                }
                self.push(line);
            }
            "_Player" => {
                // Another player's default group made one of the computer player's (Eden 4's ally's is its fifth).
                if this != ai
                    && method == "SetDefaultGroup"
                    && let Some(p) = this.strip_prefix("Player").and_then(|p| p.parse::<i32>().ok())
                    && let Ok(g) = self.group(a.get(1).map_or("", String::as_str))
                {
                    self.push(format!("players[{}]:set_default_group({g})", p + 1));
                    return Ok(());
                }
                if this != ai {
                    return Ok(());
                }
                let who = if self.top { "p" } else { "ai_player" };
                let v = a.get(1).cloned().unwrap_or_default();
                let field = |f: &str| format!("{who}.{f} = {v}");
                let line = match method {
                    "SetOre" => field("ore"),
                    "SetRareOre" => field("rare_ore"),
                    "SetFoodStored" => field("food"),
                    "SetWorkers" => field("workers"),
                    "SetScientists" => field("scientists"),
                    "SetKids" => field("kids"),
                    "SetTechLevel" => format!("{who}:set_tech_level({v})"),
                    "MarkResearchComplete" => format!("{who}:mark_research({v})"),
                    "GoEden" => format!("{who}:go_eden()"),
                    "GoPlymouth" => format!("{who}:go_plymouth()"),
                    "GoAI" => format!("{who}:go_ai()"),
                    // Allied with another player (Eden 4's two computer players): "this, n", 0-based.
                    "AllyWith" => format!("{who}:ally_with(players[{}])", v.trim().parse::<i32>().map_err(|_| format!("AllyWith({v})"))? + 1),
                    "SetDefaultGroup" => format!("{who}:set_default_group({})", self.group(&v)?),
                    // Reads, and calls the Lua has no need of (the colour is the lobby's or a colony game's
                    // placement's).
                    "Difficulty" | "IsHuman" | "IsEden" | "IsPlymouth" | "IsAI" | "CenterViewOn" | "SetColorNumber" | "_Player" | "~_Player" => return Ok(()),
                    other => return Err(format!("it calls _Player::{other} on the computer player")),
                };
                self.push(line);
            }
            "TethysGame" if method == "CreateUnit" => {
                // "Unit#n, TYPE, (x,y), owner, WEAPON|None, rotation"
                let [_, ty, loc, owner, weapon, rot] = &a[..] else { return Err(format!("CreateUnit({})", a.join(", "))) };
                if owner.parse::<i32>().ok() != Some(self.ai.player) {
                    return Ok(());
                }
                // A tile the setup drew in a rectangle just before (CES1's guards): drawn the same way.
                let tile = match self.rand_pt.take() {
                    Some((rect, at)) if ints(&at) == ints(loc) => format!("game.rand_point({})", self.ai.rect(&rect)?),
                    _ => self.ai.at(loc)?,
                };
                let mut f = vec![format!("type = {}", type_arg(ty)), format!("player = {}", if self.top { "p" } else { "ai_player" }), format!("at = {tile}")];
                if weapon != "None" {
                    f.push(format!("weapon = {}", type_arg(weapon)));
                }
                let r = rot.parse::<usize>().unwrap_or(0) % 8;
                if r != 0 {
                    f.push(format!("direction = \"{}\"", DIRECTIONS[r]));
                }
                // The setup's CreateUnit puts a unit on the tile itself, vehicles too (OP2Lua's finder would move them:
                // CPS2's truck at x 1262, which the game puts at the map's east edge).
                f.push("exact = true".into());
                // With its lights off, as CreateUnit makes it (OP2Lua's default is on); a DoSetLights after switches them.
                f.push("lights = false".into());
                // Held until the next call shows whether a group takes it.
                self.flush();
                self.pending = Some(format!("game.create_unit{{ {} }}", f.join(", ")));
            }
            "TethysGame" if method == "CreateWallOrTube" => {} // the setup's tubes are in placement.lua
            "MAP_RECT" if method == "RandPt" => self.rand_pt = Some((this.to_string(), c.ret.clone())),
            // "enabled, one_shot, time[, max], \"Name\"": a trigger the function makes (CES1's SetupBase2 starts
            // Base2CommonCheck); the function it calls is converted too.
            "" if method == "CreateTimeTrigger" && !self.top => {
                if let Some(id) = hash_id(&c.ret) {
                    let callee = a.last().map(|n| n.trim_matches('"').to_string()).unwrap_or_default();
                    self.ai.triggers.insert(id, callee);
                }
                let name = a.last().map(|n| n.trim_matches('"').to_string()).ok_or("CreateTimeTrigger without a function")?;
                let n: Vec<i64> = a[..a.len() - 1].iter().filter_map(|v| v.trim().parse().ok()).collect();
                let fields = match n[..] {
                    [e, o, t] => crate::timed::timer_fields(e != 0, o != 0, t, None),
                    [e, o, lo, hi] => crate::timed::timer_fields(e != 0, o != 0, lo, Some(hi)),
                    _ => return Err(format!("CreateTimeTrigger({})", a.join(", "))),
                };
                self.calls.insert(name.clone());
                self.push(format!("trigger.time{{{fields}, on = {name}}}"));
            }
            // "Player1, \"Name\", lights": the units of the DLL's exported table `Name`, in no group.
            "TethysGame" if method == "CreateUnitBlock" => {
                if a.first() != Some(&ai) {
                    return Ok(());
                }
                let name = a.get(1).map(|n| n.trim_matches('"').to_string()).unwrap_or_default();
                let at = *self.ai.named.get(&name).ok_or_else(|| format!("CreateUnitBlock of {name}"))?;
                let block = self.ai.block(&format!("0x{at:x}"))?;
                let lights = a.get(2).is_some_and(|v| v != "0");
                let who = if self.top { "p" } else { "ai_player" };
                self.push(format!("{block}:create_units({who}, {lights})"));
            }
            "GroupEnumerator" => {
                if method == "GroupEnumerator" {
                    let g = a.get(1).map(|s| self.group(s)).transpose();
                    self.walking = g.ok().flatten();
                    self.lookup = false;
                }
            }
            "Unit" => {
                let id = hash_id(this).ok_or_else(|| format!("{api} on {this}"))?;
                // Reads: what they lead to shows in the calls after (the tiles a later call is given).
                if matches!(method, "GetType" | "Location" | "IsLive") {
                    return Ok(());
                }
                let Some((owner, code)) = self.ai.units.get(&id).cloned() else { return Ok(()) };
                if owner != self.ai.player {
                    return Ok(());
                }
                // A call on the unit just made (Eden 11's guards fire back, SetOppFiredUpon): the unit becomes a local,
                // the call is made on it, and the group takes it after.
                if let Some(p) = self.pending.clone().filter(|p| p.starts_with("game.create_unit{")) {
                    let var = format!("u{id}");
                    self.pending = None;
                    self.out.push(format!("local {var} = {p}[1]"));
                    self.pending = Some(format!("{{ {var} }}"));
                    self.made_units.insert(id, var);
                }
                if let Some(var) = self.made_units.get(&id).cloned() {
                    let line = match method {
                        "SetOppFiredUpon" => format!("{var}:set_opp_fired_upon({})", a.get(1).is_some_and(|v| v != "0")),
                        "SetDamage" => format!("{var}:set_damage({})", a.get(1).cloned().unwrap_or_default()),
                        "DoIdle" => format!("{var}:idle()"),
                        "DoSetLights" => format!("{var}:set_lights({})", a.get(1).is_some_and(|v| v != "0")),
                        other => return Err(format!("it calls Unit::{other} on a computer unit")),
                    };
                    self.out.push(line);
                    return Ok(());
                }
                // Otherwise the unit is one of a group's, picked from it by type.
                let line = match method {
                    "SetFactoryCargo" => {
                        let [_, bay, kit, extra] = &a[..] else { return Err(format!("SetFactoryCargo({})", a.join(", "))) };
                        let bay: i64 = bay.parse().map_err(|_| format!("a bay {bay}"))?;
                        let extra = if extra == "None" { String::new() } else { format!(", {}", type_arg(extra)) };
                        format!("set_factory_cargo({}, {}{extra})", bay + 1, type_arg(kit))
                    }
                    "DoSetLights" => format!("set_lights({})", a.get(1).is_some_and(|v| v != "0")),
                    other => return Err(format!("it calls Unit::{other} on a computer unit")),
                };
                if self.picked.as_ref().map(|(p, _)| *p) != Some(id) {
                    let walking = self.walking.clone().ok_or("a unit call outside a walk of a group")?;
                    let same = self.ai.units.values().filter(|(o, c)| *o == self.ai.player && *c == code).count();
                    // A lookup gives the group's first of the type, as first_of_type does, however many there are.
                    if same != 1 && !self.lookup {
                        return Err(format!("a call on one of the computer player's {same} {code}s"));
                    }
                    // The local is the type's capitals (`sf` for a StructureFactory); the calls after go in its `if`.
                    let var = lua_name(map_id(&code)).chars().filter(char::is_ascii_uppercase).collect::<String>().to_lowercase();
                    self.flush();
                    self.out.push(format!("local {var} = {walking}:first_of_type({})", type_arg(&code)));
                    self.out.push(format!("if {var} then"));
                    self.picked = Some((id, var));
                }
                let var = self.picked.as_ref().map(|(_, v)| v.clone()).expect("picked");
                self.out.push(format!("{var}:{line}"));
            }
            "ScStub" => match method {
                "~ScStub" | "ScStub" => {}
                "Destroy" => {
                    let id = hash_id(this).ok_or("Destroy of nothing")?;
                    if let Some(g) = self.made.get(&id).or_else(|| self.ai.groups.get(&id)).cloned() {
                        self.push(format!("{g}:destroy()"));
                    } else if self.me.is_some() && self.ai.triggers.get(&id).map(String::as_str) == self.me {
                        self.push("t:destroy()".into());
                    } else {
                        return Err(format!("it ends stub #{id}, not its own trigger"));
                    }
                }
                other => return Err(format!("it calls ScStub::{other}")),
            },
            "ScGroup" | "BuildingGroup" | "FightGroup" | "MiningGroup" => {
                let g = self.group(this)?;
                let rest = &a[1.min(a.len())..];
                let arg = |i: usize| rest.get(i).cloned().unwrap_or_default();
                // A unit looked up in the group (Eden 11's Structure Factory, by its class): the calls on it after
                // pick it from the group as a walk of it does.
                if method == "GetFirstOfType" {
                    self.walking = Some(g);
                    self.lookup = true;
                    return Ok(());
                }
                let call = match method {
                    "TakeUnit" => {
                        let u = self.pending.take().ok_or_else(|| format!("{g} takes a unit made elsewhere ({})", arg(0)))?;
                        // A unit a later call names (Eden 11's guards, SetOppFiredUpon after the group has them): kept.
                        match hash_id(&arg(0)).filter(|id| self.named_later.contains(id) && !self.made_units.contains_key(id)) {
                            Some(id) => {
                                let var = format!("u{id}");
                                self.out.push(format!("local {var} = {u}[1]"));
                                self.made_units.insert(id, var.clone());
                                format!("take_unit({var})")
                            }
                            None => format!("take_unit({u}[1])"),
                        }
                    }
                    "AddUnits" => format!("add_units({})", self.ai.block(&arg(0))?),
                    "SetTargCount" if rest.len() == 3 => format!("set_targ_count({}, {}, {})", type_arg(&arg(0)), type_arg(&arg(1)), arg(2)),
                    // The counts of a unit table (Eden 11's guards): each type and weapon as many as the table has.
                    "SetTargCount" if rest.len() == 1 => format!("set_targ_counts({})", self.ai.block(&arg(0))?),
                    "ClearTargCount" => "clear_targ_count()".into(),
                    "SetLights" => format!("set_lights({})", arg(0) != "0"),
                    "SetDeleteWhenEmpty" => format!("set_delete_when_empty({})", arg(0) != "0"),
                    "SetRect" => format!("set_rect({})", self.ai.rect(&arg(0))?),
                    "RecordUnitBlock" => match rest.len() {
                        1 => format!("record_unit_block({})", self.ai.block(&arg(0))?),
                        _ => format!("record_unit_block({}, {})", self.ai.block(&arg(0))?, self.group(&arg(1))?),
                    },
                    "RecordBuilding" => {
                        let mut f = vec![self.ai.at(&arg(0))?, type_arg(&arg(1))];
                        if rest.len() >= 4 {
                            f.push(type_arg(&arg(2)));
                            f.push(self.group(&arg(3))?);
                        } else if arg(2) != "None" {
                            f.push(type_arg(&arg(2)));
                        }
                        format!("record_building({})", f.join(", "))
                    }
                    "RecordTube" => format!("record_tube({})", self.ai.at(&arg(0))?),
                    "RecordTubesTouching" | "RecordConnectTubes" => format!("record_tubes_touching({})", self.ai.at(&arg(0))?),
                    "RecordWall" => match arg(1).as_str() {
                        "WALL" | "" => format!("record_wall({})", self.ai.at(&arg(0))?),
                        w => format!("record_wall({}, {})", self.ai.at(&arg(0))?, type_arg(w)),
                    },
                    "RecordVehReinforceGroup" => format!("record_veh_reinforce_group({}, {})", self.group(&arg(0))?, arg(1)),
                    "UnRecordVehGroup" => format!("unrecord_veh_group({})", self.group(&arg(0))?),
                    "Setup" if rest.len() == 3 && rest[0].starts_with('(') => {
                        format!("setup({}, {}, {})", self.ai.at(&arg(0))?, self.ai.at(&arg(1))?, self.ai.rect(&arg(2))?)
                    }
                    // The mine and smelter as units the setup made (Plymouth 10): found on their tiles.
                    "Setup" if rest.len() == 3 && rest[0].starts_with("Unit#") => {
                        let unit = |s: &str| -> Result<String, String> {
                            let id = hash_id(s).ok_or_else(|| format!("a unit {s}"))?;
                            let (_, code) = self.ai.units.get(&id).ok_or_else(|| format!("a unit the setup didn't make ({s})"))?;
                            let (x, y) = self.ai.unit_tiles[&id];
                            Ok(format!("unit_at({}, {{ {}, {} }})", type_arg(code), x + 1, y + 1))
                        };
                        let (mine, smelter) = (unit(&arg(0))?, unit(&arg(1))?);
                        self.ai.unit_at = true;
                        format!("setup({mine}, {smelter}, {})", self.ai.rect(&arg(2))?)
                    }
                    "Setup" if rest.len() == 5 => format!(
                        "setup({}, {}, {}, {}, {})",
                        self.ai.at(&arg(0))?,
                        self.ai.at(&arg(1))?,
                        type_arg(&arg(2)),
                        type_arg(&arg(3)),
                        self.ai.rect(&arg(4))?
                    ),
                    "DoGuardRect" => "do_guard_rect()".into(),
                    "DoPatrolOnly" => "do_patrol_only()".into(),
                    "DoGuardGroup" => "do_guard_group()".into(),
                    "DoAttackEnemy" => "do_attack_enemy()".into(),
                    "DoExitMap" => "do_exit_map()".into(),
                    "ClearGuarderdRects" | "ClearGuardedRects" => "clear_guarded_rects()".into(),
                    "AddGuardedRect" => format!("add_guarded_rect({})", self.ai.rect(&arg(0))?),
                    "SetAttackType" => format!("set_attack_type({})", type_arg(&arg(0))),
                    "SetTargetGroup" => format!("set_target_group({})", self.group(&arg(0))?),
                    "SetFollowMode" => format!("set_follow_mode({})", arg(0)),
                    // "PatrolRoute@0x...": the route's first field, then up to 8 waypoints ended by an x of -1.
                    "SetPatrolMode" => {
                        let at = address(&arg(0)).ok_or_else(|| format!("a patrol route {}", arg(0)))?;
                        let m = &self.ai.image.m;
                        let first = m.read_u32(at) as i32;
                        let points = m.read_u32(at + 4);
                        let mut route = Vec::new();
                        for i in 0..8 {
                            let (x, y) = (m.read_u32(points + 8 * i) as i32, m.read_u32(points + 8 * i + 4) as i32);
                            if x == -1 {
                                break;
                            }
                            route.push(format!("{{ {}, {} }}", x - self.ai.pad + 1, y + 1));
                        }
                        format!("set_patrol({{ {} }}, {first})", route.join(", "))
                    }
                    "SetCombineFire" => "set_combine_fire()".into(),
                    "ClearCombineFire" => "set_combine_fire(false)".into(),
                    other => return Err(format!("it calls {class}::{other}({})", rest.join(", "))),
                };
                self.push(format!("{g}:{call}"));
            }
            _ => {}
        }
        Ok(())
    }

    /// The lines as Lua, each after `indent`, a level deeper inside each `if ... then`.
    fn finish(mut self, indent: &str) -> String {
        self.flush();
        let mut s = String::new();
        // Only the `if`s of `first_of_type` picks nest, each closed by an "end" line.
        let mut depth = 0usize;
        for l in &self.out {
            if l == "end" {
                depth = depth.saturating_sub(1);
            }
            let _ = writeln!(s, "{indent}{}{l}", "  ".repeat(depth));
            if l.starts_with("if ") && l.ends_with(" then") {
                depth += 1;
            }
        }
        s
    }
}

/// The computer player of `dll` from a setup with `players` humans (it is the next player), or `None` when it has
/// none (or why it can't be converted).
pub fn setup(dll: &Path, players: u8, pad: i32) -> Result<Option<Ai>, String> {
    setup_with(dll, &crate::calls::disasters_on(players, 1), players as i32, pad, i32::MAX)
}

/// The computer player `ai` (0-based) of `dll` from a setup run with `opts`, `None` when the setup makes it no
/// groups. A tile whose x is beyond `width` (the map's, in file tiles) is written as the game's own (`raw`).
pub fn setup_with(dll: &Path, opts: &Options, ai: i32, pad: i32, width: i32) -> Result<Option<Ai>, String> {
    setup_or_empty(dll, opts, ai, pad, width, false)
}

/// [`setup_with`]; with `empty`, a computer player whose setup makes no groups is still one, with an empty setup, for
/// the trigger functions that make its groups later (a campaign mission's waves: Eden 7's Wave1Arrive).
pub fn setup_or_empty(dll: &Path, opts: &Options, ai: i32, pad: i32, width: i32, empty: bool) -> Result<Option<Ai>, String> {
    let (setup, calls) = crate::setup::run_with(dll, opts).map_err(|e| e.to_string())?;
    let groups = calls.iter().any(|c| c.api.starts_with("Create") && c.api.ends_with("Group") && c.args.first() == Some(&format!("Player{ai}")));
    if !groups && !empty {
        return Ok(None);
    }
    // Each UnitBlock's table, from the DllMain constructor calls; the tables as the DLL ships them.
    let image = Mission::load(dll, &Options { call_log: false, with_sim: false, ..Options::default() }).map_err(|e| e.to_string())?;
    let mut blocks = BTreeMap::new();
    for c in calls.iter().filter(|c| c.api == "UnitBlock::UnitBlock") {
        let a = args(c);
        let (Some(b), Some(t)) = (a.first().and_then(|s| address(s)), a.get(1).and_then(|s| address(s))) else { continue };
        let recs = records(&image, t);
        // A record's cargo (its last field) isn't written to `AI_UNITS`.
        if recs.iter().any(|r| r[7] != 0) {
            return Err("a unit table with cargo".into());
        }
        blocks.insert(b, recs);
    }
    // The exported unit blocks a `CreateUnitBlock` names (CES1's "AIInitialBase").
    let mut named = BTreeMap::new();
    let data = std::fs::read(dll).map_err(|e| e.to_string())?;
    let pe = op2_x86::pe::PeFile::parse(&data).map_err(|e| format!("{e:?}"))?;
    for c in calls.iter().filter(|c| c.api == "TethysGame::CreateUnitBlock") {
        let Some(name) = args(c).get(1).map(|n| n.trim_matches('"').to_string()) else { continue };
        let Some(e) = pe.exports.iter().find(|e| e.name.as_deref() == Some(name.as_str())) else {
            return Err(format!("CreateUnitBlock names {name}, which the DLL doesn't export"));
        };
        // The export is the UnitBlock the DLL's start built (CES1's AIInitialBase at 0x110182A8).
        let a = pe.image_base + e.rva;
        if !blocks.contains_key(&a) {
            return Err(format!("CreateUnitBlock names {name}, which isn't a unit block the DLL's start built"));
        }
        named.insert(name, a);
    }
    let mut ai = Ai {
        player: ai,
        pad,
        width,
        groups: BTreeMap::new(),
        blocks,
        named,
        used: Vec::new(),
        triggers: setup.triggers.iter().filter_map(|t| Some((t.id as i64, t.callback.clone()?))).collect(),
        units: setup.objects.iter().map(|o| (o.id as i64, (o.owner, o.type_name.clone()))).collect(),
        unit_tiles: setup.objects.iter().map(|o| (o.id as i64, o.tile)).collect(),
        unit_at: false,
        setup: String::new(),
        counters: Vec::new(),
        level_groups: Vec::new(),
        image,
    };
    if !groups {
        return Ok(Some(ai));
    }
    // The setup's calls in order; a unit a later `Unit::` call names is kept as a local.
    let named_later = unit_ids(calls.iter().filter(|c| c.phase == "InitProc"));
    let mut lines = Lines { ai: &mut ai, made: BTreeMap::new(), top: true, me: None, out: Vec::new(), pending: None, made_units: BTreeMap::new(), named_later, lookup: false, walking: None, picked: None, rand_pt: None, calls: BTreeSet::new() };
    for c in calls.iter().filter(|c| c.phase == "InitProc") {
        lines.call(c).map_err(|why| format!("the computer player's setup: {why}"))?;
    }
    let body = lines.finish("  ");
    ai.setup = body;
    Ok(Some(ai))
}

/// The units of the UnitBlock at `block` as the game holds them after the DLL's start (the constructor sorts its
/// table: the count at +0, the records at the pointer at +0x84, 32 bytes each), in the order a mission's loops over
/// them read them, as `ai.unit_block` records (`op2dll disasm --block`). Tiles on a map padded by `pad` (32, or 0 on a
/// 512-wide map).
pub fn block_lua(dll: &Path, difficulty: i32, block: u32, pad: i32) -> Result<String, String> {
    let opts = Options { call_log: false, difficulty, ..Options::from_env() };
    let head = crate::calls::peek(dll, &opts, block, 34).map_err(|e| e.to_string())?;
    // Dword 33 is the pointer at +0x84.
    let (count, records) = (head[0] as usize, head[33]);
    if count == 0 || count > 256 || records == 0 {
        return Err(format!("no UnitBlock at 0x{block:08X}"));
    }
    let raw = crate::calls::peek(dll, &opts, records, 8 * count).map_err(|e| e.to_string())?;
    let mut s = String::from("{
");
    // Each record written as `header` writes `AI_UNITS`' records.
    for r in raw.chunks(8) {
        let r: Vec<i32> = r.iter().map(|v| *v as i32).collect();
        let mut f = vec![format!("type = {}", quoted(&lua_name(r[0]))), format!("at = {{ {}, {} }}", r[1] - pad + 1, r[2] + 1)];
        match r[4] {
            0 => {}
            rot if rot % 32 == 0 && (0..256).contains(&rot) => f.push(format!("direction = \"{}\"", DIRECTIONS[(rot / 32) as usize])),
            rot => f.push(format!("rotation = {rot}")),
        }
        if r[5] != 0 {
            f.push(format!("weapon = {}", quoted(&lua_name(r[5]))));
        }
        if r[6] != 16 {
            f.push(format!("classification = {}", r[6]));
        }
        let _ = writeln!(s, "  {{ {} }},", f.join(", "));
    }
    s.push_str("}
");
    Ok(s)
}

/// The units of the UnitBlock at `block` as the DLL ships its table, before the constructor sorts it in place: what
/// `ai.unit_block` takes to make the same block, the game sorting it as the mission's own constructor did (`op2dll
/// disasm --block --shipped`). Tiles on a map padded by `pad`, as in [`block_lua`].
pub fn shipped_block_lua(dll: &Path, block: u32, pad: i32) -> Result<String, String> {
    let opts = Options { call_log: true, ..Options::from_env() };
    let (_, calls) = crate::setup::run_with(dll, &opts).map_err(|e| e.to_string())?;
    let table = calls
        .iter()
        .filter(|c| c.api == "UnitBlock::UnitBlock")
        .find_map(|c| {
            let a = args(c);
            (a.first().and_then(|s| address(s)) == Some(block)).then(|| a.get(1).and_then(|s| address(s))).flatten()
        })
        .ok_or_else(|| format!("no UnitBlock the DLL's start built at 0x{block:08X}"))?;
    let image = Mission::load(dll, &Options { call_log: false, with_sim: false, ..Options::default() }).map_err(|e| e.to_string())?;
    let mut s = String::from("{
");
    for r in records(&image, table) {
        let mut f = vec![format!("type = {}", quoted(&lua_name(r[0]))), format!("at = {{ {}, {} }}", r[1] - pad + 1, r[2] + 1)];
        match r[4] {
            0 => {}
            rot if rot % 32 == 0 && (0..256).contains(&rot) => f.push(format!("direction = \"{}\"", DIRECTIONS[(rot / 32) as usize])),
            rot => f.push(format!("rotation = {rot}")),
        }
        if r[5] != 0 {
            f.push(format!("weapon = {}", quoted(&lua_name(r[5]))));
        }
        if r[6] != 16 {
            f.push(format!("classification = {}", r[6]));
        }
        let _ = writeln!(s, "  {{ {} }},", f.join(", "));
    }
    s.push_str("}
");
    Ok(s)
}

/// The PatrolRoute at `at`: its waypoints (up to 8, ended by an x of -1) and first field, as `set_patrol`'s arguments
/// (`op2dll disasm --route`). Tiles on a map padded by `pad`, as in [`block_lua`].
pub fn route_lua(dll: &Path, at: u32, pad: i32) -> Result<String, String> {
    let image = Mission::load(dll, &Options { call_log: false, with_sim: false, ..Options::default() }).map_err(|e| e.to_string())?;
    let m = &image.m;
    let first = m.read_u32(at) as i32;
    let points = m.read_u32(at + 4);
    let mut route = Vec::new();
    for i in 0..8 {
        let (x, y) = (m.read_u32(points + 8 * i) as i32, m.read_u32(points + 8 * i + 4) as i32);
        if x == -1 {
            break;
        }
        route.push(format!("{{ {}, {} }}", x - pad + 1, y + 1));
    }
    Ok(format!("{{ {} }}, {first}\n", route.join(", ")))
}

/// The Lua body of the trigger function `name` from `runs` of it (each its calls), when it only works on the computer
/// player's groups, and the functions the triggers it makes call. Every run alike is one body; runs that differ and
/// then settle on one ending for at least three runs are steps on a counter (`<name>_step`); otherwise an error.
pub fn function(ai: &mut Ai, name: &str, runs: &[Vec<CallRec>]) -> Result<(String, BTreeSet<String>), String> {
    // A function whose first call ends the trigger that called it runs once (CES1's SetupBase2): later calls of these
    // runs end whatever trigger then has its number.
    let own = |c: &CallRec| {
        c.api == "ScStub::Destroy"
            && args(c).first().and_then(|t| t.strip_prefix("this=")).and_then(hash_id).is_some_and(|id| ai.triggers.get(&id).map(String::as_str) == Some(name))
    };
    let runs = if runs.first().is_some_and(|r| r.iter().any(own)) { &runs[..1] } else { runs };
    let mut bodies = Vec::new();
    let mut calls = BTreeSet::new();
    for run in runs {
        let named_later = unit_ids(run.iter());
        let mut lines = Lines { ai: &mut *ai, made: BTreeMap::new(), top: false, me: Some(name), out: Vec::new(), pending: None, made_units: BTreeMap::new(), named_later, lookup: false, walking: None, picked: None, rand_pt: None, calls: BTreeSet::new() };
        for c in run {
            lines.call(c)?;
        }
        calls.extend(std::mem::take(&mut lines.calls));
        bodies.push(lines.finish(""));
    }
    let indent = |b: &str, by: &str| b.lines().map(|l| format!("{by}{l}\n")).collect::<String>();
    if bodies.windows(2).all(|w| w[0] == w[1]) {
        return Ok((indent(&bodies[0], "  "), calls));
    }
    // Steps: the calls differ, then repeat one ending (an empty ending writes no `else`).
    let last = bodies.last().expect("runs");
    let first_same = bodies.iter().rposition(|b| b != last).map_or(0, |i| i + 1);
    if runs.len() - first_same < 3 {
        return Err(format!("its calls differ from one call to the next and don't settle within {} calls", runs.len()));
    }
    let counter = format!("{name}_step");
    ai.counters.push(counter.clone());
    let mut s = String::new();
    let _ = writeln!(s, "  {counter} = {counter} + 1");
    for (i, b) in bodies[..first_same].iter().enumerate() {
        let _ = writeln!(s, "  {} {counter} == {} then", if i == 0 { "if" } else { "elseif" }, i + 1);
        s.push_str(&indent(b, "    "));
    }
    if !last.trim().is_empty() {
        let _ = writeln!(s, "  else");
        s.push_str(&indent(last, "    "));
    }
    let _ = writeln!(s, "  end");
    Ok((s, calls))
}

/// The ids of the units `Unit::` calls name.
fn unit_ids<'a>(calls: impl Iterator<Item = &'a CallRec>) -> BTreeSet<i64> {
    calls
        .filter(|c| c.api.starts_with("Unit::"))
        .filter_map(|c| args(c).first().and_then(|t| t.strip_prefix("this=")).and_then(hash_id))
        .collect()
}

/// Whether a run calls anything of the computer player's groups.
pub fn uses_groups(run: &[CallRec]) -> bool {
    run.iter().any(|c| {
        let class = c.api.split("::").next().unwrap_or("");
        matches!(class, "ScGroup" | "BuildingGroup" | "FightGroup" | "MiningGroup") || (c.api.starts_with("Create") && c.api.ends_with("Group"))
    })
}

/// `text` with each identifier `f` names a replacement for replaced (a word starting with a digit is a number, left
/// as it is). Strings are not skipped.
fn rename_words(text: &str, f: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut word = String::new();
    for ch in text.chars().chain(std::iter::once('\0')) {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            word.push(ch);
            continue;
        }
        if !word.is_empty() {
            let starts_ident = !word.starts_with(|c: char| c.is_ascii_digit());
            match starts_ident.then(|| f(&word)).flatten() {
                Some(r) => out.push_str(&r),
                None => out.push_str(&word),
            }
            word.clear();
        }
        if ch != '\0' {
            out.push(ch);
        }
    }
    out
}
