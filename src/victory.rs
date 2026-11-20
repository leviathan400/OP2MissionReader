//! The win of a converted mission (`op2dll lua`): its victory and failure conditions and the triggers they wait on,
//! as OP2Lua's `trigger.*` and `mission.victory` calls, from the triggers op2-missionhost recorded after the setup.
//!
//! - Last One Standing (a one-player-left trigger in a victory, both on) becomes `mission.last_one_standing(text)`.
//! - Space Race, Resource Race and Midas (count, resource, set and Midas triggers that call nothing) become the same
//!   triggers made from Lua, each in its victory condition.
//! - Land Rush: a Last One Standing victory made switched off, and per player a Command Center, colonist and vehicle
//!   trigger that call the mission's `Base<N>`. What `Base<N>` does is measured by calling each one after the setup
//!   (`land_rush`): the first call for a player frees their morale if the lobby left it free, and once every player's
//!   has come it switches the victory on and posts "New Objective". The Lua does the same in a function of its own.
//!
//! - A single-player mission's victory and failure triggers: made with their player by number, and those that call a
//!   function (the campaign's evacuations) call it from Lua; [`Win::callbacks`] hands those functions to timed.rs.
//!
//! Other triggers that call mission functions (disasters, lava, scripted events) are timed.rs's; a multiplayer win
//! whose triggers call a function isn't converted, and the caller notes it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use op2_missionhost::api::{Compare, TriggerSpec};
use op2_missionhost::runtime::CallRec;

use crate::luaexport::{lua_name, quoted};
use crate::triggers::Trigger;

/// The function names of a trigger that calls nothing: the stock DLLs' and the one newer SDKs (OPU's maps) use.
pub(crate) const NO_RESPONSE: [&str; 2] = ["NoResponseToTrigger", "None"];
/// `TriggerResource`'s names, by the game's numbering. 7 isn't in Tethys's list but the stock Labyrinth (`ml2_11`)
/// uses it; op2-missionhost reads it as ore. OP2Lua's "Ore" passes the 7 on as it is.
const RESOURCES: [&str; 8] = ["Food", "CommonOre", "RareOre", "Kids", "Workers", "Scientists", "Colonists", "Ore"];

/// The win's Lua (lines for `on_init`, where `n` is the player count) and the triggers it covers.
pub struct Win {
    /// Lines for `on_init`, under a "The win." comment; empty when nothing was converted.
    pub lua: String,
    /// The setup's triggers covered: the conditions and the triggers they wait on.
    pub handled: BTreeSet<u32>,
    /// The functions the win's triggers call (a single-player mission's: Eden 1's evacuations post a message), for
    /// the timed conversion to convert or stub.
    pub callbacks: BTreeSet<String>,
}

/// What a Land Rush mission's `Base<N>` functions did when called after the setup with morale free: `Base1`, `Base1`
/// again, then `Base2` up to the last player's (op2dll's `calls --call`).
pub struct LandRush {
    /// The calls those functions made, each with its function as the phase.
    pub calls: Vec<CallRec>,
    /// The player count of the game they were measured in.
    pub players: u8,
}

/// OP2Lua's name for a trigger's comparison.
fn compare(c: Compare) -> &'static str {
    match c {
        Compare::Equal => "==",
        Compare::LowerEqual => "<=",
        Compare::GreaterEqual => ">=",
        Compare::Lower => "<",
        Compare::Greater => ">",
    }
}

/// The game's default `one_shot` for each of OP2Lua's trigger constructors (Tethys's).
fn one_shot_default(spec: &TriggerSpec) -> bool {
    matches!(spec, TriggerSpec::Midas { .. } | TriggerSpec::OnePlayerLeft | TriggerSpec::Operational { .. } | TriggerSpec::Time { .. })
}

/// A trigger's `trigger.*` call with `player` as given (Lua: a 1-based number or a variable) and `extra` fields, or
/// why it can't be made.
pub(crate) fn constructor(t: &Trigger, player: Option<&str>, names: &BTreeMap<u32, String>, extra: &str) -> Result<String, String> {
    let mut f: Vec<String> = Vec::new();
    let player_of = |p: i32| -> Result<Option<String>, String> {
        match (p, player) {
            (-1, _) => Ok(None),
            (_, Some(v)) => Ok(Some(format!("player = {v}"))),
            (p, None) => Err(format!("it watches player {} only", p + 1)),
        }
    };
    let kind = match &t.spec {
        TriggerSpec::Count { player: p, ty, cargo, count, cmp } => {
            f.push(format!("type = {}", quoted(&lua_name(ty.0))));
            if *cargo >= 0 {
                // A Cargo Truck's cargo is its truck cargo, passed as the mission's number (Eden 1's trucks of food,
                // 1, and Common Metals, 4); any other type's is a map id, passed by OP2Lua's name.
                if lua_name(ty.0) == "CargoTruck" {
                    f.push(format!("cargo = {cargo}"));
                } else {
                    f.push(format!("cargo = {}", quoted(&lua_name(*cargo))));
                }
            }
            f.push(format!("compare = {}", quoted(compare(*cmp))));
            f.push(format!("count = {count}"));
            f.extend(player_of(*p)?);
            "count"
        }
        TriggerSpec::Resource { res, amount, player: p, cmp } => {
            let name = RESOURCES.get(*res as usize).ok_or_else(|| format!("resource {res}"))?;
            f.push(format!("resource = {}", quoted(name)));
            f.push(format!("compare = {}", quoted(compare(*cmp))));
            f.push(format!("amount = {amount}"));
            f.extend(player_of(*p)?);
            "resource"
        }
        TriggerSpec::Set { members, needed } => {
            for m in members {
                f.push(names.get(&m.0).cloned().ok_or_else(|| format!("its member trigger #{} isn't converted", m.0))?);
            }
            if *needed as usize != members.len() {
                f.push(format!("needed = {needed}"));
            }
            "set"
        }
        TriggerSpec::Research { tech, player: p } => {
            f.push(format!("tech = {tech}"));
            f.extend(player_of(*p)?);
            "research"
        }
        TriggerSpec::Midas { time } => {
            f.push(format!("time = {time}"));
            "midas"
        }
        TriggerSpec::OnePlayerLeft => "one_player_left",
        TriggerSpec::Time { min, max, random } => {
            if *random && max > min {
                f.push(format!("min = {min}, max = {max}"));
            } else {
                f.push(format!("time = {min}"));
            }
            "time"
        }
        TriggerSpec::Operational { player: p, ty, count, cmp } => {
            f.push(format!("type = {}", quoted(&lua_name(ty.0))));
            f.push(format!("compare = {}", quoted(compare(*cmp))));
            f.push(format!("count = {count}"));
            f.extend(player_of(*p)?);
            "operational"
        }
        TriggerSpec::VehicleCount { player: p, count, cmp } | TriggerSpec::BuildingCount { player: p, count, cmp } => {
            f.push(format!("compare = {}", quoted(compare(*cmp))));
            f.push(format!("count = {count}"));
            f.extend(player_of(*p)?);
            if matches!(t.spec, TriggerSpec::VehicleCount { .. }) { "vehicle_count" } else { "building_count" }
        }
        // The campaign's: tiles as the mission passes them (the game's own, `raw`), a rectangle as its corner and size
        // (the mission passes x, y, width and height; `Rect` holds the corners).
        TriggerSpec::Kit { player: p, ty, count } => {
            f.push(format!("type = {}", quoted(&lua_name(ty.0))));
            f.push(format!("count = {count}"));
            f.extend(player_of(*p)?);
            "kit"
        }
        TriggerSpec::Escape { player: p, rect, count, ty, cargo, amount } => {
            f.push(format!("at = {{ x = {}, y = {}, raw = true }}, width = {}, height = {}", rect.x1, rect.y1, rect.x2 - rect.x1 + 1, rect.y2 - rect.y1 + 1));
            f.push(format!("type = {}", quoted(&lua_name(ty.0))));
            f.push(format!("count = {count}"));
            // The cargo, when the mission gives one, and the amount to reach, as the mission's numbers.
            if *cargo >= 0 {
                f.push(format!("cargo = {cargo}, amount = {amount}"));
            }
            f.extend(player_of(*p)?);
            "escape"
        }
        TriggerSpec::Evac { player: p } => {
            f.extend(player_of(*p)?);
            "evac"
        }
        TriggerSpec::Point { player: p, at } => {
            f.push(format!("at = {{ x = {}, y = {}, raw = true }}", at.x, at.y));
            f.extend(player_of(*p)?);
            "point"
        }
        TriggerSpec::InRect { player: p, rect } => {
            f.push(format!("at = {{ x = {}, y = {}, raw = true }}, width = {}, height = {}", rect.x1, rect.y1, rect.x2 - rect.x1 + 1, rect.y2 - rect.y1 + 1));
            f.extend(player_of(*p)?);
            "rect"
        }
        other => return Err(format!("{} isn't converted", other.kind_name())),
    };
    if t.one_shot != one_shot_default(&t.spec) {
        f.push(format!("one_shot = {}", t.one_shot));
    }
    if !t.enabled {
        f.push("enabled = false".into());
    }
    if !extra.is_empty() {
        f.push(extra.into());
    }
    Ok(if f.is_empty() { format!("trigger.{kind}{{}}") } else { format!("trigger.{kind}{{{}}}", f.join(", ")) })
}

/// [`constructor`], with a trigger that watches one player naming that player by number (`players[n]` is the game's
/// player n - 1), as a single-player mission's do.
pub(crate) fn constructor_numbered(t: &Trigger, names: &BTreeMap<u32, String>, extra: &str) -> Result<String, String> {
    constructor(t, None, names, extra).or_else(|why| match why.strip_prefix("it watches player ").and_then(|r| r.strip_suffix(" only")) {
        Some(n) => constructor(t, Some(n), names, extra),
        None => Err(why),
    })
}

/// The options table of `mission.victory`/`failure` where they differ from its defaults (one-shot off, enabled).
fn condition_opts(t: &Trigger, victory: bool) -> String {
    let mut f = Vec::new();
    if victory && t.one_shot {
        f.push("one_shot = true".to_string());
    }
    if !t.enabled {
        f.push("enabled = false".into());
    }
    if f.is_empty() { String::new() } else { format!(", {{{}}}", f.join(", ")) }
}

/// The triggers a condition waits on, deepest first (set members before the set), or `None` if one isn't there.
fn chain(id: u32, by_id: &BTreeMap<u32, &Trigger>, out: &mut Vec<u32>) -> Option<()> {
    let t = by_id.get(&id)?;
    if let TriggerSpec::Set { members, .. } = &t.spec {
        for m in members {
            chain(m.0, by_id, out)?;
        }
    }
    if !out.contains(&id) {
        out.push(id);
    }
    Some(())
}

/// Whether a trigger calls no function: none named, an empty name or one of [`NO_RESPONSE`].
fn calls_nothing(t: &Trigger) -> bool {
    t.callback.as_deref().is_none_or(|c| c.is_empty() || NO_RESPONSE.contains(&c))
}

/// The player number of a Land Rush trigger function's name (`Base3` -> 3).
fn base_number(name: &str) -> Option<u8> {
    name.strip_prefix("Base")?.parse().ok().filter(|n| *n >= 1)
}

/// The win of the setup's `triggers`: each victory and failure condition and the triggers it waits on, with what
/// isn't converted in `notes`. `land_rush` is what the `Base<N>` functions did, for a Land Rush mission. `numbered`: a
/// single-player mission, whose triggers on one player name that player by number and may call a function.
pub fn convert(triggers: &[Trigger], land_rush: Option<&LandRush>, numbered: bool, notes: &mut Vec<String>) -> Win {
    let by_id: BTreeMap<u32, &Trigger> = triggers.iter().map(|t| (t.id, t)).collect();
    let mut lua = String::new();
    let mut handled = BTreeSet::new();
    // The Lua local each converted trigger is made as (`t<id>`), so a set or a second condition can name it.
    let mut names: BTreeMap<u32, String> = BTreeMap::new();
    let mut body = String::new();
    let mut callbacks = BTreeSet::new();

    for cond in triggers {
        let (linked, text, victory) = match &cond.spec {
            TriggerSpec::Victory { linked, objective } => (linked.0, objective.clone(), true),
            TriggerSpec::Failure { linked, .. } => (linked.0, String::new(), false),
            _ => continue,
        };
        let Some(target) = by_id.get(&linked) else {
            notes.push(format!("condition #{} waits on trigger #{linked}, which the setup didn't keep", cond.id));
            continue;
        };
        // Last One Standing, as mission.last_one_standing makes it.
        if victory && matches!(target.spec, TriggerSpec::OnePlayerLeft) && target.enabled && cond.enabled && target.one_shot && cond.one_shot {
            let _ = writeln!(body, "  mission.last_one_standing({})", quoted(&text));
            handled.extend([cond.id, linked]);
            continue;
        }
        // Land Rush: the same victory made switched off, for the Base<N> functions to switch on.
        if victory && matches!(target.spec, TriggerSpec::OnePlayerLeft) && !target.enabled && !cond.enabled {
            match land_rush_lua(triggers, &text, land_rush) {
                Ok((code, ids)) => {
                    body.push_str(&code);
                    handled.extend([cond.id, linked]);
                    handled.extend(ids);
                }
                Err(why) => notes.push(format!("the Land Rush win isn't converted: {why}")),
            }
            continue;
        }
        let mut ids = Vec::new();
        if chain(linked, &by_id, &mut ids).is_none() {
            notes.push(format!("condition #{} waits on a trigger the setup didn't keep", cond.id));
            continue;
        }
        // A multiplayer win whose triggers call a function isn't converted. A single-player mission's may (the
        // campaign's evacuations): the trigger is made below, calling it, and the function is converted with the timed
        // ones (or made empty).
        if let Some(t) = ids.iter().map(|i| by_id[i]).find(|t| !calls_nothing(t)).filter(|_| !numbered) {
            notes.push(format!("condition #{}'s trigger #{} calls {} (not converted)", cond.id, t.id, t.callback.as_deref().unwrap_or("")));
            continue;
        }
        let mut code = String::new();
        let mut ok = true;
        for id in &ids {
            if names.contains_key(id) {
                continue;
            }
            let on = by_id[id].callback.as_deref().filter(|_| !calls_nothing(by_id[id])).map(|c| format!("on = {c}")).unwrap_or_default();
            let made = if numbered { constructor_numbered(by_id[id], &names, &on) } else { constructor(by_id[id], None, &names, &on) };
            match made {
                Ok(c) => {
                    let name = format!("t{id}");
                    let _ = writeln!(code, "  local {name} = {c}");
                    names.insert(*id, name);
                }
                Err(why) => {
                    notes.push(format!("condition #{}'s trigger #{id}: {why}", cond.id));
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        body.push_str(&code);
        let opts = condition_opts(cond, victory);
        if victory {
            let _ = writeln!(body, "  mission.victory({}, {}{opts})", names[&linked], quoted(&text));
        } else {
            let _ = writeln!(body, "  mission.failure({}{opts})", names[&linked]);
        }
        if let Some(t) = ids.iter().map(|i| by_id[i]).find(|t| !t.enabled) {
            notes.push(format!("condition #{}'s trigger #{} is made off, and what switches it on isn't converted: this win never comes", cond.id, t.id));
        }
        handled.insert(cond.id);
        // The functions these triggers call, for timed.rs to convert or make empty.
        callbacks.extend(ids.iter().filter_map(|i| by_id[i].callback.clone().filter(|_| !calls_nothing(by_id[i]))));
        handled.extend(ids);
    }
    if !body.is_empty() {
        lua.push_str("\n  -- The win.\n");
        lua.push_str(&body);
    }
    Win { lua, handled, callbacks }
}

/// The Land Rush win's Lua and the triggers it covers, after checking `Base<N>` did what it's converted as.
fn land_rush_lua(triggers: &[Trigger], text: &str, measured: Option<&LandRush>) -> Result<(String, Vec<u32>), String> {
    let lr = measured.ok_or("the mission's Base<N> functions weren't measured")?;
    // Each player's triggers that call Base<N>, by N; player 1's are the pattern for all.
    let mut per_player: BTreeMap<u8, Vec<&Trigger>> = BTreeMap::new();
    for t in triggers {
        if let Some(n) = t.callback.as_deref().and_then(base_number) {
            per_player.entry(n).or_default().push(t);
        }
    }
    if per_player.len() != lr.players as usize || per_player.keys().copied().ne(1..=lr.players) {
        return Err(format!("{} players but Base functions for {:?}", lr.players, per_player.keys().collect::<Vec<_>>()));
    }
    // A trigger as the Lua loop makes it: for the loop's player `id`, calling its `settle`.
    let shape = |t: &Trigger| constructor(t, Some("id"), &BTreeMap::new(), "on = settle");
    let pattern: Vec<String> = per_player[&1].iter().map(|t| shape(t)).collect::<Result<_, _>>()?;
    for (n, ts) in &per_player {
        // Every player's triggers are player 1's, for that player.
        let player = |t: &Trigger| match &t.spec {
            TriggerSpec::Operational { player, .. } | TriggerSpec::Resource { player, .. } | TriggerSpec::VehicleCount { player, .. } => *player,
            _ => -2,
        };
        let same = ts.len() == pattern.len()
            && ts.iter().zip(&pattern).all(|(t, p)| player(t) == i32::from(*n) - 1 && shape(t).as_ref() == Ok(p));
        if !same {
            return Err(format!("player {n}'s Base triggers differ from player 1's"));
        }
    }
    // What the calls did: per player once (a second Base1 does nothing), the last one switches the win on.
    let by_phase = |name: &str| lr.calls.iter().filter(|c| c.phase == name).map(|c| c.api.as_str()).collect::<Vec<_>>();
    let first: Vec<&CallRec> = lr.calls.iter().filter(|c| c.phase == "Base1").collect();
    // Every player's first call: is morale on, free this player's, how many players are there.
    let once = ["TethysGame::UsesMorale", "TethysGame::FreeMoraleLevel", "TethysGame::NoPlayers"];
    // Base1's calls, both runs together, are `once` alone: the second run made none.
    let twice_ok = first.len() == once.len() && first.iter().map(|c| c.api.as_str()).eq(once);
    let last = format!("Base{}", lr.players);
    let last_calls: Vec<&CallRec> = lr.calls.iter().filter(|c| c.phase == last).collect();
    // The last player's call goes on: a stub's `IsEnabled` check, two `Trigger::Enable`s, then the message.
    let tail = ["ScStub::IsEnabled", "Trigger::Enable", "Trigger::Enable", "TethysGame::AddMessage"];
    let last_ok = by_phase(&last).len() == once.len() + tail.len() && by_phase(&last).iter().copied().eq(once.iter().copied().chain(tail));
    let middle_ok = (2..lr.players).all(|n| by_phase(&format!("Base{n}")) == once);
    let freed_ok = (1..=lr.players).all(|n| {
        lr.calls.iter().any(|c| c.phase == format!("Base{n}") && c.api == "TethysGame::FreeMoraleLevel" && c.args.first().map(String::as_str) == Some(&(n - 1).to_string()))
    });
    if !(twice_ok && last_ok && middle_ok && freed_ok) {
        return Err("its Base<N> functions do more than free morale and switch the win on".into());
    }
    // The message call's arguments: its text is the third, its sound the fifth.
    let message = last_calls.last().map(|c| c.args.clone()).unwrap_or_default();
    let (said, sound) = (message.get(2).map_or("\"New Objective\"", String::as_str), message.get(4).map_or("0", String::as_str));

    let mut m = String::new();
    let _ = writeln!(m, "  -- Land Rush: Last One Standing, switched on once every player has settled (a working Command Center)");
    let _ = writeln!(m, "  -- or is out of the rush (fewer than 5 colonists, or no vehicles). A player's first such moment frees");
    let _ = writeln!(m, "  -- their morale when the lobby left it free.");
    let _ = writeln!(m, "  local last_one = trigger.one_player_left{{enabled = false}}");
    let _ = writeln!(m, "  local victory = mission.victory(last_one, {}, {{one_shot = true, enabled = false}})", quoted(text));
    let _ = writeln!(m, "  local settled, count = {{}}, 0");
    let _ = writeln!(m, "  for id = 1, n do");
    let _ = writeln!(m, "    local function settle()");
    let _ = writeln!(m, "      if settled[id] then return end");
    let _ = writeln!(m, "      settled[id] = true");
    let _ = writeln!(m, "      count = count + 1");
    let _ = writeln!(m, "      if game.uses_morale() then game.free_morale(id) end");
    let _ = writeln!(m, "      if count == n and not last_one.enabled then");
    let _ = writeln!(m, "        last_one:enable()");
    let _ = writeln!(m, "        victory:enable()");
    let _ = writeln!(m, "        game.message({said}, {sound})");
    let _ = writeln!(m, "      end");
    let _ = writeln!(m, "    end");
    for p in &pattern {
        let _ = writeln!(m, "    {p}");
    }
    let _ = writeln!(m, "  end");
    let ids = per_player.values().flatten().map(|t| t.id).collect();
    Ok((m, ids))
}
