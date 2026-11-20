//! The triggers a mission's setup creates (op2-missionhost's `StubRec`s with a `TriggerSpec`), described for a reader:
//! victory and failure conditions with what they wait on, and the triggers that drive the mission's script.

use std::collections::BTreeMap;

use op2_missionhost::api::{Compare, TriggerSpec};
use op2_missionhost::names;
use op2_missionhost::world::World;
use serde::Serialize;

use crate::setup::Object;

/// The Cargo Truck's map id.
const TRUCK: i32 = 1;

/// One trigger the mission made, described for a reader.
#[derive(Debug, Clone, Serialize)]
pub struct Trigger {
    /// The game's stub index (triggers and groups share it).
    pub id: u32,
    /// The legacy class name (`VictoryCondition`, `TimeTrigger`, `RectTrigger`...).
    pub kind: String,
    /// What it watches, in words.
    pub what: String,
    pub enabled: bool,
    /// It fires once and is gone, rather than each time its condition holds.
    pub one_shot: bool,
    /// The mission function it calls when it fires.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback: Option<String>,
    /// A victory or failure condition's objective text and the trigger it waits on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objective: Option<Objective>,
    /// The file tiles it watches (top-left and bottom-right, inclusive) for rectangle, point and escape triggers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area: Option<((i32, i32), (i32, i32))>,
    /// What it watches, as op2-missionhost recorded it (for `op2dll lua`).
    #[serde(skip)]
    pub spec: TriggerSpec,
}

/// A victory or failure condition's side of a trigger.
#[derive(Debug, Clone, Serialize)]
pub struct Objective {
    /// A victory condition; false for a failure condition.
    pub victory: bool,
    /// The objective text the game lists.
    pub text: String,
    /// The id of the trigger it waits on.
    pub linked: u32,
}

/// The setup's triggers, in creation order. `techs` names research topics by id.
pub fn collect(w: &World, objects: &[Object], techs: &BTreeMap<i32, String>) -> Vec<Trigger> {
    let pad = w.pad;
    w.stubs
        .iter()
        .filter(|s| !s.destroyed)
        .filter_map(|s| {
            // Only stubs with a trigger spec are triggers (groups share the stub list).
            let spec = s.spec.as_ref()?;
            // A game tile as a file tile.
            let tile = |x: i32, y: i32| (x - pad, y);
            let (what, objective, area) = match spec {
                TriggerSpec::Victory { linked, objective } | TriggerSpec::Failure { linked, objective } => (
                    format!("when trigger #{} fires", linked.0),
                    Some(Objective { victory: matches!(spec, TriggerSpec::Victory { .. }), text: objective.clone(), linked: linked.0 }),
                    None,
                ),
                TriggerSpec::InRect { rect, .. } | TriggerSpec::Escape { rect, .. } => {
                    (describe(spec, w, objects, techs), None, Some((tile(rect.x1, rect.y1), tile(rect.x2, rect.y2))))
                }
                TriggerSpec::Point { at, .. } => (describe(spec, w, objects, techs), None, Some((tile(at.x, at.y), tile(at.x, at.y)))),
                _ => (describe(spec, w, objects, techs), None, None),
            };
            Some(Trigger {
                id: s.id,
                kind: spec.kind_name(),
                what,
                enabled: s.enabled,
                one_shot: s.one_shot,
                callback: s.callback.clone(),
                objective,
                area,
                spec: spec.clone(),
            })
        })
        .collect()
}

/// What a trigger waits on, in words, with tiles in file coordinates and research by name from `techs`.
fn describe(spec: &TriggerSpec, w: &World, objects: &[Object], techs: &BTreeMap<i32, String>) -> String {
    let pad = w.pad;
    let who = |p: i32| if p < 0 { "any player".to_string() } else { format!("player {p}") };
    // The cargo is a Cargo Truck's load code, else a weapon or kit map id; -1 or 0 is any.
    let unit = |ty: i32, cargo: i32| {
        let mut s = names::map_id_name(ty);
        if cargo > 0 {
            let what = if ty == TRUCK { crate::setup::truck_cargo(cargo as i64).to_string() } else { names::map_id_name(cargo) };
            s += &format!(" ({what})");
        }
        s
    };
    let group = |g: u32| match w.stubs.get(g as usize) {
        Some(s) => format!("group #{g} ({} units)", s.units.len()),
        None => format!("group #{g}"),
    };
    match spec {
        TriggerSpec::Time { min, max, random } => {
            if *random && max > min { format!("every {} to {} (drawn once)", time(*min), time(*max)) } else { format!("every {}", time(*min)) }
        }
        TriggerSpec::Set { members, needed } => {
            let list: Vec<String> = members.iter().map(|m| format!("#{}", m.0)).collect();
            format!("{needed} of {} have fired", list.join(", "))
        }
        TriggerSpec::Resource { res, amount, player, cmp } => {
            /// `TriggerResource`'s names, by the game's numbering.
            const RES: [&str; 8] = ["food", "common ore", "rare ore", "kids", "workers", "scientists", "colonists", "ore"];
            let res = RES.get(*res as usize).copied().unwrap_or("?");
            // For all players the comparison is always "at least" (0x4923D0).
            let cmp = if *player < 0 { Compare::GreaterEqual } else { *cmp };
            format!("{} has {res} {} {amount}", who(*player), op(cmp))
        }
        TriggerSpec::Research { tech, player } => {
            let name = techs.get(tech).map_or_else(|| format!("topic {tech}"), |n| format!("{n} ({tech})"));
            format!("{} has researched {name}", who(*player))
        }
        TriggerSpec::Count { player, ty, cargo, count, cmp } => {
            format!("{} has {} {count} {}", who(*player), op(*cmp), unit(ty.0, *cargo))
        }
        TriggerSpec::VehicleCount { player, count, cmp } => format!("{} has {} {count} vehicles", who(*player), op(*cmp)),
        TriggerSpec::BuildingCount { player, count, cmp } => format!("{} has {} {count} structures", who(*player), op(*cmp)),
        TriggerSpec::Operational { player, ty, count, cmp } => {
            format!("{} has {} {count} working {}", who(*player), op(*cmp), names::map_id_name(ty.0))
        }
        TriggerSpec::Kit { player, ty, count } => format!("{} has at least {count} {} kits", who(*player), names::map_id_name(ty.0)),
        TriggerSpec::Point { player, at } => format!("a vehicle of {} at tile {}, {}", who(*player), at.x - pad, at.y),
        TriggerSpec::InRect { player, rect } => {
            format!("a vehicle of {} in tiles {}, {} to {}, {}", who(*player), rect.x1 - pad, rect.y1, rect.x2 - pad, rect.y2)
        }
        TriggerSpec::Escape { player, rect, count, ty, cargo, amount } => {
            let mut s = format!("{} has {count} {} in tiles {}, {} to {}, {}", who(*player), unit(ty.0, *cargo), rect.x1 - pad, rect.y1, rect.x2 - pad, rect.y2);
            if *amount > 0 && ty.0 == TRUCK {
                s += &format!(", carrying {amount} in all");
            }
            s
        }
        TriggerSpec::Damaged { group: g, damage } => {
            // The mission's damage code: the share of the group destroyed.
            let share = match damage {
                1 => "all",
                2 => "75%",
                3 => "50%",
                _ => "?",
            };
            format!("{share} of {} destroyed", group(g.0))
        }
        TriggerSpec::Attacked { group: g } => format!("{} is attacked", group(g.0)),
        TriggerSpec::SpecialTarget { target, source } => {
            let target = objects.iter().find(|o| o.id == target.0).map_or_else(|| format!("unit {}", target.0), |o| format!("{} at {}, {}", o.type_name, o.tile.0, o.tile.1));
            format!("a {} waits beside the {target}", names::map_id_name(source.0))
        }
        TriggerSpec::Midas { time } => format!("Midas: after {}", self::time(*time)),
        TriggerSpec::OnePlayerLeft => "one player is left".into(),
        TriggerSpec::Evac { player } => format!("{} has evacuated its colonists", who(*player)),
        TriggerSpec::Victory { linked, .. } | TriggerSpec::Failure { linked, .. } => format!("when trigger #{} fires", linked.0),
        TriggerSpec::Other { kind, params } => format!("{kind} {params:?}"),
    }
}

/// Game time in marks (100 ticks each), with the ticks left over as hundredths: 69999 ticks is "699.99 marks".
fn time(ticks: i32) -> String {
    let (marks, rest) = (ticks / 100, (ticks % 100).abs());
    match (marks, rest) {
        (1, 0) => "1 mark".into(),
        (_, 0) => format!("{marks} marks"),
        _ => format!("{}.{} marks", marks, format!("{rest:02}").trim_end_matches('0')),
    }
}

/// A count comparison in words.
fn op(c: Compare) -> &'static str {
    match c {
        Compare::Equal => "exactly",
        Compare::LowerEqual => "at most",
        Compare::GreaterEqual => "at least",
        Compare::Lower => "fewer than",
        Compare::Greater => "more than",
    }
}
