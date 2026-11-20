//! The AI groups a mission's setup creates for computer players (FightGroup, MiningGroup, BuildingGroup): what each is
//! told, its units, the units it wants, and a building group's plans (the structures, mines, walls and tubes it
//! rebuilds). Read from op2-sim's groups after `InitProc`; groups made later by triggers or `AIProc` aren't here.

use std::collections::BTreeMap;

use op2_missionhost::names;
use op2_missionhost::world::World;
use op2_sim::groups::{GroupKind, Objective, PxRect};
use serde::Serialize;

use crate::setup::Object;

/// `mapAny`: a target count's weapon (or a ConVec's kit) that any will do.
const ANY: i32 = 115;

/// A rectangle of file tiles, top-left and bottom-right, inclusive.
pub type Area = ((i32, i32), (i32, i32));

/// One AI group from the setup, described for a reader.
#[derive(Debug, Clone, Serialize)]
pub struct Group {
    /// The game's stub index (groups and triggers share it).
    pub id: u32,
    /// `Fight`, `Mining` or `Building`.
    pub kind: &'static str,
    /// The player it belongs to (0-based).
    pub owner: i32,
    pub enabled: bool,
    /// Its units, by object id.
    pub members: Vec<u32>,
    /// What it's told, as rows of a label and a value.
    pub orders: Vec<(&'static str, String)>,
    /// SetTargCount: the units it wants, and how many of them it has.
    pub wants: Vec<Want>,
    /// The tiles it keeps to, guards or patrols, labelled.
    pub areas: Vec<(String, Area)>,
    /// A building group's recorded structures, mines and wall or tube tiles.
    pub plans: Vec<Plan>,
    /// The mission's calls on it, as op2-missionhost logged them.
    pub calls: Vec<String>,
}

/// One `SetTargCount` entry: a unit type the group wants so many of.
#[derive(Debug, Clone, Serialize)]
pub struct Want {
    /// The unit type, with its weapon or kit when one is asked for.
    pub unit: String,
    /// How many it wants.
    pub count: i32,
    /// How many of its members match.
    pub have: usize,
}

/// One thing a building group keeps built: a structure, a mine, or a wall or tube tile.
#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    /// The unit type (with its weapon), or the wall or tube.
    pub what: String,
    /// Where it goes: a structure's or mine's recorded tiles, a wall's or tube's one tile.
    pub area: Area,
    /// Built there now (a structure of that type stands on its tile).
    pub standing: bool,
    /// A wall or tube tile rather than a structure.
    pub wall: bool,
}

impl Group {
    /// "#3 Fight group".
    pub fn title(&self) -> String {
        format!("#{} {} group", self.id, self.kind)
    }

    /// The tiles to centre on: its first area, else its first plan.
    pub fn focus(&self) -> Option<Area> {
        self.areas.first().map(|a| a.1).or_else(|| self.plans.first().map(|p| p.area))
    }
}

/// The setup's groups, in creation order.
pub fn collect(w: &World, objects: &[Object]) -> Vec<Group> {
    let Some(sim) = w.sim.as_ref() else { return Vec::new() };
    // op2-sim's unit ids to the objects'.
    let by_sim: BTreeMap<_, _> = w.live_units().filter_map(|u| Some((u.sim?, u.id))).collect();
    // An op2-sim unit's object, and its description ("(gone)" when it no longer stands).
    let object = |id| by_sim.get(&id).and_then(|id| objects.iter().find(|o| o.id == *id));
    let named = |id| object(id).map_or_else(|| "(gone)".to_string(), |o| format!("{} at {}, {}", o.type_name, o.tile.0, o.tile.1));
    let unit = |map_id: i32, weapon: i32| {
        if weapon > 0 && weapon != ANY { format!("{} ({})", names::map_id_name(map_id), names::map_id_name(weapon)) } else { names::map_id_name(map_id) }
    };
    // A rectangle of file pixels as the file tiles it touches (32 pixels a tile).
    let tiles = |r: &PxRect| ((r.x1.div_euclid(32), r.y1.div_euclid(32)), (r.x2.div_euclid(32), r.y2.div_euclid(32)));
    sim.groups()
        .list
        .iter()
        // The mission's groups: those a harness script made have a name.
        .filter(|g| g.name.is_empty())
        .map(|g| {
            let members: Vec<&Object> = g.members.iter().filter_map(|m| object(m.unit)).collect();
            let mut orders = Vec::new();
            let mut areas = Vec::new();
            let mut plans = Vec::new();
            match g.kind {
                GroupKind::Fight => {
                    let f = &g.fight;
                    let objective = match f.objective {
                        Objective::GuardRect => "guard its area",
                        Objective::Patrol => "patrol",
                        Objective::GuardGroup => "guard a group",
                        Objective::GuardUnit => "guard a unit",
                        Objective::AttackUnit => "attack a unit",
                        Objective::AttackEnemy => "attack the enemy",
                        Objective::ExitMap => "leave the map",
                    };
                    orders.push(("Objective", objective.to_string()));
                    if let Some(u) = f.target_unit {
                        orders.push(("Target unit", named(u)));
                    }
                    if let Some(t) = f.target_group {
                        orders.push(("Target group", format!("#{t}")));
                    }
                    if f.attack_type > 0 {
                        orders.push(("Attacks first", names::map_id_name(f.attack_type)));
                    }
                    if f.combine_fire {
                        orders.push(("Combined fire", "on".into()));
                    }
                    if let Some(r) = &f.rect {
                        areas.push(("area".to_string(), tiles(r)));
                    }
                    // The rectangles it guards, other than the one already listed as its area.
                    for r in f.guarded.iter().filter(|r| Some(*r) != f.rect.as_ref()) {
                        areas.push(("guards".to_string(), tiles(r)));
                    }
                    if f.patrol || !f.waypoints.is_empty() {
                        let route: Vec<String> = f.waypoints.iter().map(|(x, y)| format!("{x}, {y}")).collect();
                        orders.push(("Patrol", route.join(" → ")));
                        for (i, &(x, y)) in f.waypoints.iter().enumerate() {
                            areas.push((format!("waypoint {}", i + 1), ((x, y), (x, y))));
                        }
                    }
                }
                GroupKind::Mining => {
                    let m = &g.mining;
                    if let Some(u) = m.mine {
                        orders.push(("Mine", named(u)));
                    }
                    if let Some(u) = m.smelter {
                        orders.push(("Smelter", named(u)));
                    }
                    if m.rect != PxRect::default() {
                        areas.push(("waits".to_string(), tiles(&m.rect)));
                    }
                }
                GroupKind::Building => {
                    let b = &g.building;
                    if b.rect != PxRect::default() {
                        areas.push(("area".to_string(), tiles(&b.rect)));
                    }
                    if !b.veh_groups.is_empty() {
                        let to: Vec<String> = b.veh_groups.iter().map(|v| format!("#{} (priority {})", v.group, v.priority)).collect();
                        orders.push(("Reinforces", to.join(", ")));
                    }
                    // Whether a structure of that type has its tile inside the rectangle now.
                    let stands = |code: &str, (x1, y1, x2, y2): (i32, i32, i32, i32)| {
                        objects.iter().any(|o| o.type_name == code && (x1..=x2).contains(&o.tile.0) && (y1..=y2).contains(&o.tile.1))
                    };
                    for r in &b.buildings {
                        let code = sim.rules().stats(r.kind).code.clone();
                        let [x1, y1, x2, y2] = r.rect;
                        let mut what = if r.weapon > 0 { format!("{code} ({})", names::map_id_name(r.weapon)) } else { code.clone() };
                        if let Some(to) = r.group {
                            what += &format!(", products to #{to}");
                        }
                        plans.push(Plan { what, area: ((x1, y1), (x2, y2)), standing: stands(&code, (x1, y1, x2, y2)), wall: false });
                    }
                    for r in &b.mines {
                        let code = sim.rules().stats(r.kind).code.clone();
                        let [x1, y1, x2, y2] = r.rect;
                        let mut what = code.clone();
                        if let Some(to) = r.group {
                            what += &format!(", mined by #{to}");
                        }
                        plans.push(Plan { what, area: ((x1, y1), (x2, y2)), standing: stands(&code, (x1, y1, x2, y2)), wall: false });
                    }
                    for r in &b.walls {
                        plans.push(Plan { what: names::map_id_name(r.map_id), area: (r.tile, r.tile), standing: false, wall: true });
                    }
                }
            }
            // The target counts' entries above 0; a member counts toward one when its type matches and, unless the
            // entry takes any weapon (none, or `mapAny`), its weapon too.
            let wants = g
                .targets
                .iter()
                .flat_map(|t| &t.entries)
                .filter(|e| e.target > 0)
                .map(|e| Want {
                    unit: unit(e.map_id, e.weapon),
                    count: e.target,
                    have: members.iter().filter(|o| o.type_id == e.map_id && (e.weapon <= 0 || e.weapon == ANY || o.weapon_id == e.weapon)).count(),
                })
                .collect();
            let stub = w.stubs.iter().find(|s| s.id == g.id);
            Group {
                id: g.id,
                kind: match g.kind {
                    GroupKind::Fight => "Fight",
                    GroupKind::Mining => "Mining",
                    GroupKind::Building => "Building",
                },
                owner: g.owner as i32,
                enabled: g.enabled,
                members: members.iter().map(|o| o.id).collect(),
                orders,
                wants,
                areas,
                plans,
                calls: stub.map(|s| s.calls.clone()).unwrap_or_default(),
            }
        })
        .collect()
}
