//! Reading a mission DLL without running any of its code: is it an Outpost 2 mission, and what does its descriptor
//! say (name, type, players, tech level, map, tech tree)?

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use op2_missionhost::runtime::{Desc, Mission, Options};
use op2_x86::pe::PeFile;
use serde::Serialize;

/// The exports the game looks up in a mission DLL. `InitProc` and a descriptor are what make it a mission.
pub const MISSION_EXPORTS: [&str; 8] =
    ["InitProc", "AIProc", "GetSaveRegions", "DescBlock", "DescBlockEx", "LevelDesc", "MapName", "TechtreeName"];

/// What [`inspect`] found in one DLL, without running any of its code.
#[derive(Debug, Serialize)]
pub struct Report {
    /// The DLL's path as given.
    pub file: PathBuf,
    /// Why this isn't an Outpost 2 mission; `None` when it is.
    pub not_a_mission: Option<String>,
    /// The names in [`MISSION_EXPORTS`] the DLL exports.
    pub mission_exports: Vec<&'static str>,
    /// The names in [`MISSION_EXPORTS`] it doesn't.
    pub missing_exports: Vec<&'static str>,
    /// Functions imported from Outpost2.exe (the game API the mission calls).
    pub game_imports: usize,
    /// The mission descriptor, read by mapping the DLL; `None` when it isn't a mission or didn't map.
    pub desc: Option<Desc>,
    /// The descriptor's mission type in words ([`mission_kind`]).
    pub kind: Option<String>,
    /// The map the descriptor names, when it was found in the game folder.
    pub map: Option<MapInfo>,
    /// Problems that don't stop the inspection (the map not found, the DLL failing to map).
    pub notes: Vec<String>,
}

/// The size and makeup of the mission's map, read from its `.map` file.
#[derive(Debug, Serialize)]
pub struct MapInfo {
    /// In tiles, as the file stores it (no 32-tile padding).
    pub width: u32,
    /// In tiles.
    pub height: u32,
    /// The tileset slots the map fills.
    pub tilesets: usize,
    /// Tile mappings (the distinct tile pictures the map can show).
    pub mappings: usize,
}

impl Report {
    /// Whether the DLL passed every check for a mission.
    pub fn is_mission(&self) -> bool {
        self.not_a_mission.is_none()
    }
}

/// Inspects one DLL. `game_dir` is where the map is looked up.
pub fn inspect(path: &Path, game_dir: Option<&Path>) -> Result<Report> {
    let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let mut r = Report {
        file: path.to_path_buf(),
        not_a_mission: None,
        mission_exports: Vec::new(),
        missing_exports: Vec::new(),
        game_imports: 0,
        desc: None,
        kind: None,
        map: None,
        notes: Vec::new(),
    };
    let pe = match PeFile::parse(&data) {
        Ok(pe) => pe,
        Err(e) => {
            r.not_a_mission = Some(format!("not a Windows DLL ({})", e.0));
            return Ok(r);
        }
    };
    let exported = |name: &str| pe.exports.iter().any(|e| e.name.as_deref() == Some(name));
    (r.mission_exports, r.missing_exports) = MISSION_EXPORTS.iter().partition(|n| exported(n));
    r.game_imports = pe.imports.iter().filter(|i| i.dll.eq_ignore_ascii_case("Outpost2.exe")).count();
    // The checks in order: 0x14C is IMAGE_FILE_MACHINE_I386, 0x2000 the PE header's IMAGE_FILE_DLL flag.
    r.not_a_mission = if pe.machine != 0x14C {
        Some(format!("not 32-bit x86 (machine {:#06x})", pe.machine))
    } else if pe.characteristics & 0x2000 == 0 {
        Some("an executable, not a DLL".into())
    } else if !exported("InitProc") {
        Some("no InitProc export".into())
    } else if !exported("DescBlock") && !exported("MapName") {
        Some("no mission descriptor (DescBlock or MapName export)".into())
    } else {
        None
    };
    if !r.is_mission() {
        return Ok(r);
    }

    // Mapping the DLL reads the descriptor; nothing of the mission runs yet.
    let opts = Options { call_log: false, with_sim: false, ..Options::default() };
    let desc = match Mission::load(path, &opts) {
        Ok(m) => m.desc.clone(),
        Err(e) => {
            r.notes.push(format!("couldn't map the DLL: {e:#}"));
            return Ok(r);
        }
    };
    r.kind = Some(mission_kind(desc.mission_type));
    if desc.map_name.is_empty() {
        r.notes.push("the descriptor names no map".into());
    } else if let Some(dir) = game_dir {
        match op2_formats::GameFiles::open(dir).and_then(|f| f.read_map(&desc.map_name)) {
            Ok(map) => {
                r.map = Some(MapInfo {
                    width: map.width,
                    height: map.height,
                    tilesets: map.tilesets.iter().flatten().count(),
                    mappings: map.mappings.len(),
                })
            }
            Err(e) => r.notes.push(format!("map {}: {e}", desc.map_name)),
        }
    }
    r.desc = Some(desc);
    Ok(r)
}

/// `DescBlock`'s mission type: positive numbers are campaign missions, negative ones the other modes (the
/// `MissionTypes` enum of the Outpost 2 mission SDK).
pub fn mission_kind(t: i32) -> String {
    match t {
        n if n > 0 => format!("campaign mission {n}"),
        0 => "unspecified (0)".into(),
        -1 => "colony game".into(),
        -2 => "auto demo".into(),
        -3 => "tutorial".into(),
        -4 => "multiplayer: Land Rush".into(),
        -5 => "multiplayer: Space Race".into(),
        -6 => "multiplayer: Resource Race".into(),
        -7 => "multiplayer: Midas".into(),
        -8 => "multiplayer: Last One Standing".into(),
        n => format!("unknown ({n})"),
    }
}

/// Prints the report as `inspect` shows one DLL.
pub fn print_full(r: &Report) {
    let name = r.file.file_name().map_or_else(|| r.file.display().to_string(), |f| f.to_string_lossy().into_owned());
    match &r.not_a_mission {
        Some(why) => {
            println!("{name}: not an Outpost 2 mission: {why}");
            return;
        }
        None => println!("{name}: Outpost 2 mission"),
    }
    if let Some(d) = &r.desc {
        let or_none = |s: &str| if s.is_empty() { "(none)".to_string() } else { s.to_string() };
        println!("  Name:          {}", or_none(&d.level_desc));
        println!("  Type:          {}", r.kind.as_deref().unwrap_or("?"));
        println!("  Players:       {}", d.num_players);
        if let Some(ai) = d.ai_players {
            println!("  AI players:    {ai}");
        }
        println!("  Tech level:    {}", d.max_tech_level);
        println!("  Unit mission:  {}", if d.unit_mission { "yes" } else { "no" });
        let map = match &r.map {
            Some(m) => format!(" ({}x{} tiles, {} tilesets)", m.width, m.height, m.tilesets),
            None => String::new(),
        };
        println!("  Map:           {}{map}", or_none(&d.map_name));
        println!("  Tech tree:     {}", or_none(&d.techtree_name));
    }
    println!("  Game API calls imported: {}", r.game_imports);
    println!("  Exports:       {}", r.mission_exports.join(", "));
    if !r.missing_exports.is_empty() {
        println!("  Not exported:  {}", r.missing_exports.join(", "));
    }
    for n in &r.notes {
        println!("  note: {n}");
    }
}

/// One line per DLL, for a whole folder.
pub fn print_line(r: &Report) {
    let name = r.file.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    match (&r.not_a_mission, &r.desc) {
        (Some(why), _) => println!("{name:<14} not a mission: {why}"),
        (None, Some(d)) => {
            let map = r.map.as_ref().map_or_else(|| "?".to_string(), |m| format!("{}x{}", m.width, m.height));
            let note = if r.notes.is_empty() { String::new() } else { format!("  [{}]", r.notes.join("; ")) };
            println!(
                "{name:<14} {:<32} {:<16} {map:<8} {}{note}",
                r.kind.as_deref().unwrap_or("?"),
                d.map_name,
                d.level_desc
            );
        }
        (None, None) => println!("{name:<14} mission, descriptor unreadable  [{}]", r.notes.join("; ")),
    }
}
