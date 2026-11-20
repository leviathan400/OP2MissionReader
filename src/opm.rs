//! Exporting a mission's starting state as an `.opm` file: the JSON mission format of OP2MissionEditor and the
//! OP2DotNetMissionSDK (OPM_FORMAT.md in op2opmtools describes it).
//!
//! What goes in is what the mission's setup made in the run shown: players and their resources, units, walls and
//! tubes, beacons, markers and wreckage, daylight and music. The file is written the way the editor writes it
//! (DataContract style: tabs, members in alphabetical order, every member present), and read back by the SDK as
//! described in its `MissionLogic`: positions are the game's coordinates (`GetMapCoordinates`), which are the map
//! file's tile plus one on both axes.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;
use op2_missionhost::names;
use op2_missionhost::runtime::Desc;
use serde::Serialize;

use crate::setup::{Kind, Object, Player as SetupPlayer, Setup};

/// The SDK's `map_id` names, by value (`DotNetMissionSDK/Outpost2DLL/MapIdEnum.cs`). 0x4B has no name.
const MAP_IDS: [&str; 0x74] = [
    "None", "CargoTruck", "ConVec", "Spider", "Scorpion", "Lynx", "Panther", "Tiger", "RoboSurveyor", "RoboMiner",
    "GeoCon", "Scout", "RoboDozer", "EvacuationTransport", "RepairVehicle", "Earthworker",
    "SmallCapacityAirTransport", "Tube", "Wall", "LavaWall", "MicrobeWall", "CommonOreMine", "RareOreMine",
    "GuardPost", "LightTower", "CommonStorage", "RareStorage", "Forum", "CommandCenter", "MHDGenerator", "Residence",
    "RobotCommand", "TradeCenter", "BasicLab", "MedicalCenter", "Nursery", "SolarPowerArray", "RecreationFacility",
    "University", "Agridome", "DIRT", "Garage", "MagmaWell", "MeteorDefense", "GeothermalPlant", "ArachnidFactory",
    "ConsumerFactory", "StructureFactory", "VehicleFactory", "StandardLab", "AdvancedLab", "Observatory",
    "ReinforcedResidence", "AdvancedResidence", "CommonOreSmelter", "Spaceport", "RareOreSmelter", "GORF", "Tokamak",
    "AcidCloud", "EMP", "Laser", "Microwave", "RailGun", "RPG", "Starflare", "Supernova", "Starflare2", "Supernova2",
    "NormalUnitExplosion", "ESG", "Stickyfoam", "ThorsHammer", "EnergyCannon", "Blast", "", "Lightning", "Vortex",
    "Earthquake", "Eruption", "Meteor", "MiningBeacon", "MagmaVent", "Fumarole", "mapWreckage",
    "DisasterousBuildingExplosion", "CatastrophicBuildingExplosion", "AtheistBuildingExplosion", "EDWARDSatellite",
    "SolarSatellite", "IonDriveModule", "FusionDriveModule", "CommandModule", "FuelingSystems", "HabitatRing",
    "SensorPackage", "Skydock", "StasisSystems", "OrbitalPackage", "PhoenixModule", "RareMetalsCargo",
    "CommonMetalsCargo", "FoodCargo", "EvacuationModule", "ChildrenModule", "SULV", "RLV", "EMPMissile",
    "ImpulseItems", "Wares", "LuxuryWares", "InterColonyShuttle", "Spider3Pack", "Scorpion3Pack", "PrettyArt",
    "GeneralUnit",
];

/// The SDK's `TruckCargo` names, by value (0..=9; 10, a unit, has none).
const TRUCK_CARGO: [&str; 10] =
    ["Empty", "Food", "CommonOre", "RareOre", "CommonMetal", "RareMetal", "CommonRubble", "RareRubble", "Spaceport", "Garbage"];

// The SDK's enum names, by value: unit direction, player colour, morale, marker type.

/// Unit directions, by eighths of a turn from east ([`Object::rotation`]).
const DIRECTIONS: [&str; 8] = ["East", "SouthEast", "South", "SouthWest", "West", "NorthWest", "North", "NorthEast"];
/// Player colours, by colour index.
const COLORS: [&str; 8] = ["Blue", "Red", "Green", "Yellow", "Cyan", "Magenta", "Black", "White"];
/// Morale levels, by [`crate::setup::PlayerStart::morale_level`] (0 best).
const MORALE: [&str; 5] = ["Excellent", "Good", "Fair", "Poor", "Terrible"];
/// Marker types, by the type the mission gave the marker (its `extra["marker"]`).
const MARKERS: [&str; 3] = ["Circle", "DNA", "Beaker"];

// The map ids the export treats specially: beacons (absorbed by a mine on them) and markers.

/// `map_id` of a mining beacon.
const MINING_BEACON: i32 = 0x51;
/// `map_id` of a marker.
const MARKER: i32 = 0x72;

// The structs mirror the SDK's DataContract classes (MissionReader/Json); members are declared in alphabetical order
// so the output matches the editor's.

/// The whole `.opm` document. Disasters, regions and triggers are always empty: the export carries none.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct MissionRoot {
    disasters: Vec<serde_json::Value>,
    level_details: LevelDetails,
    /// Everything the setup made; `mission_variants` stays empty.
    master_variant: Variant,
    mission_variants: Vec<Variant>,
    regions: Vec<serde_json::Value>,
    #[serde(rename = "SDKVersion")]
    sdk_version: String,
    triggers: Vec<serde_json::Value>,
}

/// The mission descriptor's fields.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct LevelDetails {
    level_description: String,
    map_name: String,
    max_tech_level: i32,
    /// [`mission_type`]'s name, or the number for a campaign mission.
    mission_type: String,
    num_players: i32,
    tech_tree_name: String,
    unit_only_mission: bool,
}

/// One variant of the mission: its players and the game-wide state. The export writes one, for the difficulty the
/// setup ran at, with no per-difficulty copies.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Variant {
    auto_layouts: Vec<serde_json::Value>,
    name: Option<String>,
    players: Vec<Player>,
    tethys_difficulties: Vec<GameData>,
    tethys_game: GameData,
}

/// One player: colony, colour, alliances and starting state.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Player {
    /// Other players' ids (the player itself left out).
    allies: Vec<i32>,
    /// Always `None`: the mission's AI is code, not something the format holds.
    bot_type: String,
    /// A [`COLORS`] name.
    color: String,
    difficulty_resources: Vec<Resources>,
    #[serde(rename = "ID")]
    id: i32,
    is_eden: bool,
    is_human: bool,
    resources: Resources,
}

/// A player's starting state, and the units and walls/tubes that are theirs.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Resources {
    /// The tile the camera starts on; (0, 0) when the player has nothing to look at.
    center_view: Point,
    common_ore: i32,
    /// Tech ids.
    completed_research: Vec<i32>,
    food: i32,
    /// Morale moves freely; false when the setup forced it (`ForceMorale...`) without freeing it again.
    free_morale: bool,
    kids: i32,
    /// A [`MORALE`] name.
    morale_level: String,
    rare_ore: i32,
    scientists: i32,
    solar_satellites: i32,
    tech_level: i32,
    triggers: Vec<serde_json::Value>,
    units: Vec<Unit>,
    wall_tubes: Vec<WallTube>,
    workers: i32,
}

/// One vehicle or structure.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Unit {
    /// A mine's beacon variant (`Variant1`..`Variant3`, `Random`); `None` for anything else.
    bar_variant: Option<String>,
    /// A mine's beacon yield (`Bar3`, `Bar2`, `Bar1`, `Random`); `None` for anything else.
    bar_yield: Option<String>,
    /// A truck's load amount, or a ConVec's kit's weapon (`cargoWeapon`, a `map_id` number).
    cargo_amount: i32,
    /// A truck's [`TRUCK_CARGO`] name, else a weapon's or kit's [`MAP_IDS`] name, else `None`.
    cargo_type: String,
    create_wall: bool,
    /// A [`DIRECTIONS`] name.
    direction: String,
    /// Hit points left, 0.0..=1.0.
    health: f32,
    #[serde(rename = "ID")]
    id: i32,
    ignore_layout: bool,
    lights: bool,
    max_tubes: Option<i32>,
    min_distance: i32,
    position: Point,
    spawn_distance: i32,
    /// A [`MAP_IDS`] name.
    #[serde(rename = "TypeID")]
    type_id: String,
}

/// One wall or tube tile.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct WallTube {
    position: Point,
    /// A [`MAP_IDS`] name (`Tube`, `Wall`, `LavaWall`, `MicrobeWall`).
    #[serde(rename = "TypeID")]
    type_id: String,
}

/// The game-wide state: beacons, markers, wreckage, daylight and music.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct GameData {
    beacons: Vec<Beacon>,
    daylight_everywhere: bool,
    daylight_moves: bool,
    initial_light_level: i32,
    markers: Vec<Marker>,
    music_play_list: MusicPlayList,
    triggers: Vec<serde_json::Value>,
    wreckage: Vec<Wreckage>,
}

/// The songs to play, by song id, and the index in that list to loop back to.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct MusicPlayList {
    repeat_start_index: u32,
    #[serde(rename = "SongIDs")]
    song_ids: Vec<u32>,
}

/// A mining beacon, magma vent or fumarole no mine stands on.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Beacon {
    /// `Variant1`..`Variant3` or `Random`.
    bar_variant: String,
    /// `Bar3`, `Bar2`, `Bar1` or `Random`, as the mission gave it.
    bar_yield: String,
    #[serde(rename = "ID")]
    id: i32,
    /// `MiningBeacon`, `MagmaVent` or `Fumarole`.
    #[serde(rename = "MapID")]
    map_id: String,
    /// `Common`, `Rare` or `Random`.
    ore_type: String,
    position: Point,
}

/// A marker (circle, DNA or beaker) the mission placed.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Marker {
    #[serde(rename = "ID")]
    id: i32,
    /// A [`MARKERS`] name.
    marker_type: String,
    position: Point,
}

/// A wreck and the tech it gives.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Wreckage {
    #[serde(rename = "ID")]
    id: i32,
    /// The wreck's `discovered` flag.
    is_visible: bool,
    position: Point,
    #[serde(rename = "TechID")]
    tech_id: i32,
}

/// A position in the SDK's coordinates: the file tile plus one on both axes ([`Point::of`]).
#[derive(Serialize, Clone, Copy)]
struct Point {
    #[serde(rename = "X")]
    x: i32,
    #[serde(rename = "Y")]
    y: i32,
}

impl Point {
    /// The SDK's position for a file tile.
    fn of(tile: (i32, i32)) -> Point {
        Point { x: tile.0 + 1, y: tile.1 + 1 }
    }
}

/// The SDK's `map_id` name for a type.
fn map_id(id: i32) -> String {
    match MAP_IDS.get(id as usize) {
        Some(n) if id >= 0 && !n.is_empty() => n.to_string(),
        // Enum.TryParse reads a number too.
        _ => id.to_string(),
    }
}

/// The name at `v`, or the number when there is none.
fn named(names: &[&str], v: i64) -> String {
    usize::try_from(v).ok().and_then(|i| names.get(i)).map_or_else(|| v.to_string(), |n| n.to_string())
}

/// `-1` random, else the name at `v`.
fn random_or(names: &[&str], v: i64) -> String {
    if v < 0 { "Random".into() } else { named(names, v) }
}

/// The SDK's `MissionType` name: colony, demo, tutorial and multiplayer types are negative.
pub(crate) fn mission_type(t: i32) -> String {
    let names = ["Colony", "AutoDemo", "Tutorial", "MultiLandRush", "MultiSpaceRace", "MultiResourceRace", "MultiMidas", "MultiLastOneStanding"];
    // Campaign missions (positive) have no name; the SDK parses the number.
    match t {
        -8..=-1 => names[(-t - 1) as usize].into(),
        _ => t.to_string(),
    }
}

/// The mission as an `.opm` document, and what couldn't be carried over.
pub fn build(desc: &Desc, setup: &Setup) -> (MissionRoot, Vec<String>) {
    let mut notes = Vec::new();
    if let Some(f) = &setup.failed {
        notes.push(format!("the setup didn't finish ({f}); only what it made before that is in the file"));
    }
    // A game-wide switch's last arguments as numbers, if the setup called it.
    let option = |name: &str| setup.options.get(name).map(|v| v.split(',').map(|a| a.trim().parse::<i32>().unwrap_or(0)).collect::<Vec<_>>());

    // Mines make their own beacon in the SDK, so the beacon under a mine goes into the mine instead.
    let mut beacon_used = BTreeSet::new();
    let beacon_at = |tile: (i32, i32)| setup.objects.iter().find(|o| o.type_id == MINING_BEACON && o.tile == tile);

    let mut players: Vec<Player> = setup.players.iter().map(|p| player(p, setup, &option)).collect();
    let (mut gaia, mut bays, mut others, mut garaged) = (0, 0, 0, 0);
    for o in &setup.objects {
        if !matches!(o.kind, Kind::Vehicle | Kind::Structure) {
            continue;
        }
        // The format has no garages; written out, the vehicle would stand on the Garage's tile.
        if o.garage.is_some() {
            garaged += 1;
            continue;
        }
        let Some(p) = players.iter_mut().find(|p| p.id == o.owner) else {
            gaia += 1;
            continue;
        };
        if o.extra.keys().any(|k| k.starts_with("bay")) {
            bays += 1;
        }
        let mut u = unit(o);
        // 0x15 and 0x16: a common or rare ore mine. The beacon's yield code counts 0 for 3 bars.
        if matches!(o.type_id, 0x15 | 0x16)
            && let Some(b) = beacon_at(o.tile)
        {
            beacon_used.insert(b.id);
            u.bar_yield = Some(random_or(&["Bar3", "Bar2", "Bar1"], b.extra.get("yield").copied().unwrap_or(-1)));
            u.bar_variant = Some(random_or(&["Variant1", "Variant2", "Variant3"], b.extra.get("variant").copied().unwrap_or(-1)));
        }
        p.resources.units.push(u);
    }

    let mut game = GameData {
        beacons: Vec::new(),
        daylight_everywhere: option("SetDaylightEverywhere").is_some_and(|a| a.first().copied().unwrap_or(0) != 0),
        // Daylight moves unless the setup turned it off.
        daylight_moves: option("SetDaylightMoves").is_none_or(|a| a.first().copied().unwrap_or(0) != 0),
        initial_light_level: 0,
        markers: Vec::new(),
        music_play_list: match &setup.music {
            Some((songs, repeat)) => MusicPlayList { repeat_start_index: *repeat, song_ids: songs.clone() },
            None => MusicPlayList { repeat_start_index: 0, song_ids: vec![0] },
        },
        triggers: Vec::new(),
        wreckage: Vec::new(),
    };
    for o in &setup.objects {
        let at = Point::of(o.tile);
        let extra = |k: &str| o.extra.get(k).copied();
        match o.kind {
            Kind::Resource if !beacon_used.contains(&o.id) => game.beacons.push(Beacon {
                bar_variant: random_or(&["Variant1", "Variant2", "Variant3"], extra("variant").unwrap_or(-1)),
                bar_yield: random_or(&["Bar3", "Bar2", "Bar1"], extra("yield").unwrap_or(-1)),
                id: 0,
                map_id: map_id(o.type_id),
                ore_type: random_or(&["Common", "Rare"], extra("oreType").unwrap_or(-1)),
                position: at,
            }),
            Kind::Wreckage => game.wreckage.push(Wreckage { id: 0, is_visible: extra("discovered").unwrap_or(0) != 0, position: at, tech_id: o.weapon_id }),
            Kind::Other if o.type_id == MARKER && o.extra.contains_key("marker") => {
                game.markers.push(Marker { id: 0, marker_type: named(&MARKERS, extra("marker").unwrap_or(0)), position: at })
            }
            Kind::Other => others += 1,
            _ => {}
        }
    }

    // The SDK only has walls and tubes per player; the game's belong to nobody, so they go with the first player.
    let walls: Vec<WallTube> = setup
        .walls
        .iter()
        .map(|w| WallTube { position: Point::of(w.tile), type_id: names::map_id_of(&w.type_name).map_or_else(|| w.type_name.clone(), map_id) })
        .collect();
    if !walls.is_empty() {
        match players.first_mut() {
            Some(p) => p.resources.wall_tubes = walls,
            None => notes.push(format!("{} walls and tubes left out: no players", walls.len())),
        }
    }

    if gaia > 0 {
        notes.push(format!("{gaia} units owned by no player (Gaia) left out: the format has no place for them"));
    }
    if bays > 0 {
        notes.push(format!("{bays} factories' or spaceports' bay contents left out: the format has no place for them"));
    }
    if garaged > 0 {
        notes.push(format!("{garaged} vehicles parked in garages left out: the format has no place for them"));
    }
    if others > 0 {
        notes.push(format!("{others} other objects (animations, disasters' objects) left out"));
    }
    if players.iter().any(|p| !p.is_human) {
        notes.push("computer players are written with BotType None: the mission's own AI code isn't in the file".into());
    }
    let scripted = setup.unimplemented.len();
    if scripted > 0 {
        notes.push(format!("{scripted} game API functions op2-rs only stubs were called; what they did is missing"));
    }

    let root = MissionRoot {
        disasters: Vec::new(),
        level_details: LevelDetails {
            level_description: desc.level_desc.clone(),
            map_name: desc.map_name.clone(),
            max_tech_level: desc.max_tech_level,
            mission_type: mission_type(desc.mission_type),
            num_players: desc.num_players,
            tech_tree_name: desc.techtree_name.clone(),
            unit_only_mission: desc.unit_mission,
        },
        master_variant: Variant { auto_layouts: Vec::new(), name: None, players, tethys_difficulties: Vec::new(), tethys_game: game },
        mission_variants: Vec::new(),
        regions: Vec::new(),
        sdk_version: "0".into(),
        triggers: Vec::new(),
    };
    (root, notes)
}

/// One player's colony, colour and starting state; `option` reads a setup option the mission set (`ForceMorale...`).
fn player(p: &SetupPlayer, setup: &Setup, option: &impl Fn(&str) -> Option<Vec<i32>>) -> Player {
    let s = &p.start;
    // A morale switch applies to this player when its first argument is the player's id, or negative (every player).
    let applies = |name: &str| option(name).is_some_and(|a| matches!(a.first(), Some(&n) if n < 0 || n == p.id));
    let forced = ["ForceMoraleGreat", "ForceMoraleGood", "ForceMoraleOK", "ForceMoralePoor", "ForceMoraleRotten"].iter().any(|n| applies(n));
    // Without a view of its own, the camera starts on the player's first structure, else its first unit.
    let mine = || setup.objects.iter().filter(|o| o.owner == p.id);
    let view = s
        .view
        .or_else(|| mine().find(|o| o.kind == Kind::Structure).map(|o| o.tile))
        .or_else(|| mine().find(|o| o.kind == Kind::Vehicle).map(|o| o.tile));
    Player {
        allies: s.allies.iter().copied().filter(|&a| a != p.id).collect(),
        bot_type: "None".into(),
        color: named(&COLORS, p.color as i64),
        difficulty_resources: Vec::new(),
        id: p.id,
        is_eden: p.eden,
        is_human: p.human,
        resources: Resources {
            center_view: view.map_or(Point { x: 0, y: 0 }, Point::of),
            common_ore: s.ore,
            completed_research: s.techs.clone(),
            food: s.food,
            free_morale: !forced || applies("FreeMoraleLevel"),
            kids: s.kids,
            morale_level: named(&MORALE, s.morale_level.clamp(0, 4) as i64),
            rare_ore: s.rare_ore,
            scientists: s.scientists,
            solar_satellites: s.solar_sats,
            tech_level: s.tech_level,
            triggers: Vec::new(),
            units: Vec::new(),
            wall_tubes: Vec::new(),
            workers: s.workers,
        },
    }
}

/// One object as an SDK unit: type, place, direction, weapon and what it holds.
fn unit(o: &Object) -> Unit {
    let extra = |k: &str| o.extra.get(k).copied();
    let (cargo_type, cargo_amount) = match o.type_id {
        // A truck's load (TruckCargo names); a unit as cargo (10) has no name, so it's written empty.
        0x01 => match extra("truckCargo").unwrap_or(0) {
            c @ 0..=9 => (named(&TRUCK_CARGO, c), extra("truckAmount").unwrap_or(0) as i32),
            _ => ("Empty".into(), 0),
        },
        // A ConVec's kit, and a Guard Post kit's weapon.
        0x02 => (if o.weapon_id > 0 { map_id(o.weapon_id) } else { "None".into() }, extra("cargoWeapon").unwrap_or(0) as i32),
        _ => (if o.weapon_id > 0 { map_id(o.weapon_id) } else { "None".into() }, 0),
    };
    Unit {
        bar_variant: None,
        bar_yield: None,
        cargo_amount,
        cargo_type,
        create_wall: false,
        direction: DIRECTIONS[o.rotation.rem_euclid(8) as usize].into(),
        health: o.health,
        id: 0,
        ignore_layout: false,
        lights: o.lights,
        max_tubes: None,
        min_distance: 0,
        position: Point::of(o.tile),
        spawn_distance: 0,
        type_id: map_id(o.type_id),
    }
}

/// Writes the mission as an `.opm` file; returns what couldn't be carried over.
pub fn save(path: &Path, desc: &Desc, setup: &Setup) -> Result<Vec<String>> {
    let (root, notes) = build(desc, setup);
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut out, serde_json::ser::PrettyFormatter::with_indent(b"\t"));
    root.serialize(&mut ser)?;
    // A CRLF after the closing brace, as the editor's files end.
    out.extend_from_slice(b"\r\n");
    std::fs::write(path, out)?;
    Ok(notes)
}
