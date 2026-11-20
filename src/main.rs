//! op2dll: opens an Outpost 2 mission DLL and shows what it sets up.
//!
//! ```text
//! op2dll [--game-dir DIR] [--center X,Y] [--screenshot OUT.png] [DLL]   the window, optionally with a mission open
//! op2dll inspect [--json] [--objects] [--difficulty 0-2] [--players N] [--game-dir DIR] DLL|FOLDER
//! op2dll render [--difficulty 0-2] [--players N] [--size PX] [--jpg] [--game-dir DIR] DLL OUT.png|.jpg | FOLDER OUTDIR
//! op2dll opm [--difficulty 0-2] [--players N] [--game-dir DIR] DLL OUT.opm
//! op2dll forecast [--json] [--marks N] [--difficulty 0-2] [--game-dir DIR] DLL
//! op2dll messages [--json] [--marks N] [--difficulty 0-2] [--players N] [--game-dir DIR] DLL
//! op2dll calls [--start start_info.json] [--marks N] [--call EXPORT] [--repeat N] [--difficulty 0-2] [--players N] [--game-dir DIR] DLL
//! op2dll disasm [--count N | --dwords N [--difficulty 0-2] | --block [--difficulty 0-2 | --shipped] | --route] [--game-dir DIR] DLL EXPORT|0xADDRESS
//! op2dll lua [--simple] [--stub LuaMission.dll] [--game-dir DIR] DLL OUTDIR
//! ```
//!
//! A DLL name that isn't a path is looked up in the game folder (`op2dll inspect e01.dll`). `inspect` on a folder
//! lists every DLL in it, one line each. `--objects` also runs each mission's setup (InitProc) and lists what it
//! creates; `--difficulty` (default 1, normal) and `--players` (multiplayer; default all the map's slots) are the
//! choices the game makes before that, and `render` takes them too to save the whole map with its objects as a PNG
//! (on a folder, one `NAME.png` per mission DLL in OUTDIR, skipping DLLs that aren't missions); `--size` shrinks it so
//! its longer side is at most PX pixels, and a `.jpg` name (or `--jpg` on a folder) saves a JPEG;
//! `opm` saves the mission's starting state as an OP2MissionEditor `.opm` file. `forecast` and `messages` replay the
//! mission with nobody playing (600 marks unless `--marks`) and print the disasters, or the message texts with what
//! posts them and the messages the replay saw. `calls` prints every game API call of the setup (and of `--marks`
//! more), from a multiplayer lobby's choices with `--start`; `--call` and `--repeat` call trigger functions after it.
//! `disasm` prints the code of an export or from a 0x address, to its end or for `--count` instructions; `--dwords` the memory at an address
//! after the setup; `--block` a UnitBlock's unit table (at `--difficulty`, or as shipped with `--shipped`) and
//! `--route` a PatrolRoute, both as Lua. `lua` converts the mission to OP2Lua's files in OUTDIR.
//! The game folder is `--game-dir`, else `$OP2_DIR`, else the one saved in op2-rs's settings, else the usual install
//! places. `--screenshot` saves the window after it has drawn and exits.

// Release builds are Windows GUI programs (no console window opens with them); `inspect` attaches to the console it
// was started from instead.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod ai;
mod app;
mod background;
mod browser;
mod calls;
mod colony;
mod disasm;
mod disasters;
mod luaexport;
mod replay;
mod victory;
mod mapview;
mod messages;
mod objects;
mod opm;
mod textures;
mod timed;
mod windows;

// The library's modules, under the names the program's own modules use (`crate::setup`).
use op2dll::{export, groups, inspect, scene, setup, sprites, triggers};

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};

/// op2dll's version, from Cargo.toml: shown by `--help` and in the window's title.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Printed for `--help` and after a command-line error; the same forms as the module comment's.
const USAGE: &str = "usage: op2dll [--game-dir DIR] [--center X,Y] [--screenshot OUT.png] [DLL]\n       \
                     op2dll inspect [--json] [--objects] [--difficulty 0-2] [--players N] [--game-dir DIR] DLL|FOLDER\n       \
                     op2dll render [--difficulty 0-2] [--players N] [--size PX] [--jpg] [--game-dir DIR] DLL OUT.png|.jpg | FOLDER OUTDIR\n       \
                     op2dll opm [--difficulty 0-2] [--players N] [--game-dir DIR] DLL OUT.opm\n       \
                     op2dll forecast [--json] [--marks N] [--difficulty 0-2] [--game-dir DIR] DLL\n       \
                     op2dll messages [--json] [--marks N] [--difficulty 0-2] [--players N] [--game-dir DIR] DLL\n       \
                     op2dll calls [--start start_info.json] [--marks N] [--call EXPORT] [--repeat N] [--difficulty 0-2] [--players N] [--game-dir DIR] DLL\n       \
                     op2dll disasm [--count N | --dwords N [--difficulty 0-2] | --block [--difficulty 0-2 | --shipped] | --route] [--game-dir DIR] DLL EXPORT|0xADDRESS\n       \
                     op2dll lua [--simple] [--stub LuaMission.dll] [--game-dir DIR] DLL OUTDIR";

fn main() -> ExitCode {
    let console = attach_console();
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("op2dll: {e:#}");
            if !console {
                // Started from Explorer or a shortcut: nobody would see the message otherwise.
                rfd::MessageDialog::new().set_title("op2dll").set_description(format!("{e:#}")).set_level(rfd::MessageLevel::Error).show();
            }
            ExitCode::FAILURE
        }
    }
}

/// Whether printed text reaches anyone: a GUI-subsystem build has no console of its own, so it borrows the one of
/// the shell that started it, if any.
#[cfg(all(windows, not(debug_assertions)))]
fn attach_console() -> bool {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: no arguments to get wrong; it only fails when there's no parent console.
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) != 0 }
}

/// Console builds (debug builds, other systems) always have the console they were started from.
#[cfg(not(all(windows, not(debug_assertions))))]
fn attach_console() -> bool {
    true
}

/// The command line: the mode and its options.
struct Args {
    /// `inspect`: the DLL's report, or one line per DLL in a folder.
    inspect: bool,
    /// `--json`: print JSON instead of text (`inspect`, `forecast`, `messages`).
    json: bool,
    /// `inspect --objects`: also run each mission's setup and list what it creates.
    objects: bool,
    /// `--difficulty`: 0 easy, 1 normal, 2 hard; `None` for the mission's default.
    difficulty: Option<i32>,
    /// `--players`: how many multiplayer slots take part (1 to 6); `None` for every slot on the map.
    players: Option<u8>,
    game_dir: Option<PathBuf>,
    /// `--screenshot`: save the window to this PNG once it has drawn, then exit.
    screenshot: Option<PathBuf>,
    /// Open the map at 100% on this file tile.
    center: Option<(i32, i32)>,
    /// The DLL or folder the mode works on (resolved against the game folder in `run`).
    target: Option<PathBuf>,
    /// `render`: save the whole map to `out`; `opm`: save the mission there as an `.opm` file.
    render: bool,
    /// `render --size`: shrink the image to this many pixels on its longer side.
    size: Option<u32>,
    /// `render --jpg` on a folder: save `NAME.jpg` instead of `NAME.png`.
    jpg: bool,
    /// `opm`: save the mission's starting state to `out` as an OP2MissionEditor file.
    opm: bool,
    /// `forecast`: the disasters in the first `marks` marks.
    forecast: bool,
    /// `messages`: the messages the mission posts in the first `marks` marks.
    messages: bool,
    /// `calls`: every game API call the setup (and the first `marks` marks) makes, from the lobby in `start`.
    calls: bool,
    /// `calls --start`: a multiplayer lobby's `start_info.json` to set the game up from.
    start: Option<PathBuf>,
    /// `calls --call NAME`: after the setup, call the mission's export NAME (a trigger function) too.
    call: Vec<String>,
    /// `calls --repeat N`: call each `--call` export N times after the setup, one run after another.
    repeat: Option<usize>,
    /// `disasm`: the code of an export, or from a 0x address (`out` holds either), to its end.
    disasm: bool,
    /// `disasm --count N`: N instructions instead of up to the function's end.
    count: Option<usize>,
    /// `disasm --dwords N`: N dwords of memory from the address after the setup (at `--difficulty`), instead of code.
    dwords: Option<usize>,
    /// `disasm --block`: the unit table of the UnitBlock at the address, as Lua, at `--difficulty`.
    block: bool,
    /// `disasm --block --shipped`: the UnitBlock's table as the DLL ships it, unsorted (what `ai.unit_block` takes).
    shipped: bool,
    /// `disasm --route`: the PatrolRoute at the address, as Lua.
    route: bool,
    /// `lua`: convert the mission to OP2Lua in the folder `out`, with OP2Lua's stub DLL from `stub`.
    lua: bool,
    /// `lua --simple`: one base layout per area, written as flat tables.
    simple: bool,
    /// `lua --stub`: OP2Lua's prebuilt `LuaMission.dll`; `None` to look for it in `lua_cli`.
    stub: Option<PathBuf>,
    /// `--marks`: how many marks (100 ticks each) to replay after the setup; `None` for the mode's default.
    marks: Option<u32>,
    /// The second plain argument: the output file or folder, or for `disasm` the export name or 0x address.
    out: Option<PathBuf>,
}

/// Reads the command line into [`Args`]: a mode word first, then its options and up to two plain arguments.
/// `None` for `--help`.
fn parse_args() -> Result<Option<Args>> {
    let mut a = Args {
        inspect: false,
        json: false,
        objects: false,
        difficulty: None,
        players: None,
        game_dir: None,
        screenshot: None,
        center: None,
        target: None,
        render: false,
        size: None,
        jpg: false,
        opm: false,
        forecast: false,
        messages: false,
        calls: false,
        start: None,
        call: Vec::new(),
        repeat: None,
        disasm: false,
        block: false,
        shipped: false,
        route: false,
        count: None,
        dwords: None,
        lua: false,
        simple: false,
        stub: None,
        marks: None,
        out: None,
    };
    let mut args = std::env::args().skip(1).peekable();
    match args.peek().map(String::as_str) {
        Some("inspect") => a.inspect = true,
        Some("render") => a.render = true,
        Some("opm") => a.opm = true,
        Some("forecast") => a.forecast = true,
        Some("messages") => a.messages = true,
        Some("calls") => a.calls = true,
        Some("lua") => a.lua = true,
        Some("disasm") => a.disasm = true,
        _ => {}
    }
    if a.inspect || a.render || a.opm || a.forecast || a.messages || a.calls || a.lua || a.disasm {
        args.next();
    }
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--json" if a.inspect || a.forecast || a.messages => a.json = true,
            "--objects" if a.inspect => a.objects = true,
            "--difficulty" => {
                let v = args.next().context("--difficulty needs 0, 1 or 2")?;
                a.difficulty = Some(v.parse().ok().filter(|d| (0..=2).contains(d)).context("--difficulty: 0 easy, 1 normal, 2 hard")?);
            }
            "--players" => {
                let v = args.next().context("--players needs a number")?;
                a.players = Some(v.parse().ok().filter(|n| (1..=6).contains(n)).context("--players: 1 to 6")?);
            }
            "--size" if a.render => {
                let v = args.next().context("--size needs a number of pixels")?;
                a.size = Some(v.parse().ok().filter(|n| (16..=16_384).contains(n)).context("--size: 16 to 16384 pixels")?);
            }
            "--jpg" if a.render => a.jpg = true,
            "--marks" if a.forecast || a.messages || a.calls => {
                let v = args.next().context("--marks needs a number")?;
                a.marks = Some(v.parse().ok().filter(|n| (0..=10_000).contains(n)).context("--marks: 0 to 10000")?);
            }
            "--simple" if a.lua => a.simple = true,
            "--stub" if a.lua => a.stub = Some(PathBuf::from(args.next().context("--stub needs OP2Lua's LuaMission.dll")?)),
            "--call" if a.calls => a.call.push(args.next().context("--call needs an export name")?),
            "--repeat" if a.calls => {
                let v = args.next().context("--repeat needs a number")?;
                a.repeat = Some(v.parse().ok().filter(|n| (1..=100_000).contains(n)).context("--repeat: 1 to 100000")?);
            }
            "--dwords" if a.disasm => {
                let v = args.next().context("--dwords needs a number")?;
                a.dwords = Some(v.parse().context("--dwords: a number of dwords")?);
            }
            "--block" if a.disasm => a.block = true,
            "--shipped" if a.disasm => a.shipped = true,
            "--route" if a.disasm => a.route = true,
            "--count" if a.disasm => {
                let v = args.next().context("--count needs a number")?;
                a.count = Some(v.parse().context("--count: a number of instructions")?);
            }
            "--start" if a.calls => a.start = Some(PathBuf::from(args.next().context("--start needs a start_info.json")?)),
            "--game-dir" => a.game_dir = Some(PathBuf::from(args.next().context("--game-dir needs a folder")?)),
            // The modes that take a second plain argument: the first goes to `target` (last arm), the second here.
            _ if (a.render || a.opm || a.lua || a.disasm) && a.target.is_some() && a.out.is_none() && !arg.starts_with("--") => a.out = Some(PathBuf::from(arg)),
            "--center" if !a.inspect => {
                let v = args.next().context("--center needs X,Y")?;
                a.center = Some(v.split_once(',').and_then(|(x, y)| Some((x.trim().parse().ok()?, y.trim().parse().ok()?))).context("--center X,Y (file tile)")?);
            }
            "--screenshot" if !a.inspect => a.screenshot = Some(PathBuf::from(args.next().context("--screenshot needs a file")?)),
            _ if a.target.is_none() && !arg.starts_with("--") => a.target = Some(PathBuf::from(arg)),
            _ => bail!("unexpected argument {arg:?}\n{USAGE}"),
        }
    }
    Ok(Some(a))
}

/// Runs the mode the command line asks for; the window when no mode is given.
fn run() -> Result<ExitCode> {
    let Some(args) = parse_args()? else {
        println!("OP2MissionReader {VERSION}\n{USAGE}");
        return Ok(ExitCode::SUCCESS);
    };
    // The game folder only matters for maps (and bare DLL names), so a missing one is a warning.
    let game_dir = match op2_formats::install::use_for_tool(args.game_dir.clone(), op2_settings::Settings::load().game_dir) {
        Ok(d) => Some(d),
        Err(e) => {
            eprintln!("warning: {e}");
            None
        }
    };
    let target = args.target.clone().map(|t| resolve(t, game_dir.as_deref())).transpose()?;
    if args.inspect {
        return inspect_cli(&target.context(USAGE)?, game_dir.as_deref(), &args);
    }
    if args.render {
        let (Some(target), Some(out)) = (&target, &args.out) else { bail!(USAGE) };
        let game_dir = game_dir.as_deref().context("render needs the game folder (--game-dir)")?;
        if target.is_dir() {
            return render_folder(target, out, game_dir, &args);
        }
        render(target, out, game_dir, &args)?;
        return Ok(ExitCode::SUCCESS);
    }
    if args.opm {
        let (Some(target), Some(out)) = (&target, &args.out) else { bail!(USAGE) };
        export_opm(target, out, &args)?;
        return Ok(ExitCode::SUCCESS);
    }
    if args.disasm {
        let (Some(target), Some(what)) = (&target, &args.out) else { bail!(USAGE) };
        let what = what.to_string_lossy();
        let hex = |s: &str| s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).and_then(|h| u32::from_str_radix(h, 16).ok());
        if args.shipped && !args.block {
            bail!("--shipped goes with --block");
        }
        if args.block || args.route {
            let addr = hex(&what).context("--block and --route need a 0x address")?;
            // Tiles as the Lua writes them: a map under 512 tiles wide is padded by 32 (none when the map isn't found).
            let wide = inspect::inspect(target, game_dir.as_deref()).ok().and_then(|r| r.map).is_some_and(|m| m.width >= 512);
            let pad = if wide { 0 } else { 32 };
            let lua = if args.block && args.shipped {
                crate::ai::shipped_block_lua(target, addr, pad)
            } else if args.block {
                crate::ai::block_lua(target, args.difficulty.unwrap_or(1), addr, pad)
            } else {
                crate::ai::route_lua(target, addr, pad)
            };
            print!("{}", lua.map_err(|e| anyhow::anyhow!(e))?);
            return Ok(ExitCode::SUCCESS);
        }
        if let Some(n) = args.dwords {
            let addr = hex(&what).context("--dwords needs a 0x address")?;
            let opts = op2_missionhost::runtime::Options {
                call_log: false,
                difficulty: args.difficulty.unwrap_or(1),
                ..op2_missionhost::runtime::Options::from_env()
            };
            for (i, v) in calls::peek(target, &opts, addr, n)?.iter().enumerate() {
                println!("{:08X}  {v:08X}  {}", addr + 4 * i as u32, *v as i32);
            }
            return Ok(ExitCode::SUCCESS);
        }
        print!("{}", disasm::disasm(target, &what, args.count)?);
        return Ok(ExitCode::SUCCESS);
    }
    if args.lua {
        let (Some(target), Some(out)) = (&target, &args.out) else { bail!(USAGE) };
        lua_cli(target, out, args.simple, args.stub.as_deref(), game_dir.as_deref())?;
        return Ok(ExitCode::SUCCESS);
    }
    if args.calls {
        let dll = target.context(USAGE)?;
        let report = inspect::inspect(&dll, None)?;
        let desc = report.desc.as_ref().with_context(|| format!("{} isn't a mission", dll.display()))?;
        let mut start = setup::StartOptions::default_for(desc.mission_type, desc.num_players);
        start.difficulty = args.difficulty.unwrap_or(start.difficulty);
        start.players = args.players.or(start.players);
        if let Some(times) = args.repeat {
            // A multiplayer mission runs in a lobby with disasters on; a single-player one at its difficulty.
            let opts = match start.players {
                Some(n) => calls::disasters_on(n, 0),
                None => op2_missionhost::runtime::Options {
                    call_log: true,
                    difficulty: start.difficulty,
                    ..op2_missionhost::runtime::Options::from_env()
                },
            };
            for name in &args.call {
                for (i, run) in calls::repeat(&dll, &opts, name, times)?.iter().enumerate() {
                    println!("--- {name} #{}", i + 1);
                    run.iter().for_each(|c| println!("{}", calls::line(c)));
                }
            }
            return Ok(ExitCode::SUCCESS);
        }
        for c in calls::record(&dll, start, args.start.as_deref(), args.marks.unwrap_or(0), &args.call)? {
            println!("{}", calls::line(&c));
        }
        return Ok(ExitCode::SUCCESS);
    }
    if args.forecast || args.messages {
        forecast_cli(&target.context(USAGE)?, &args)?;
        return Ok(ExitCode::SUCCESS);
    }

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]).with_title(format!("OP2MissionReader {VERSION}")).with_icon(app::icon()),
        ..Default::default()
    };
    let (screenshot, center) = (args.screenshot.clone().map(app::ScreenshotJob::new), args.center);
    eframe::run_native("op2dll", options, Box::new(|cc| Ok(Box::new(app::App::new(cc, game_dir, target, center, screenshot)))))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(ExitCode::SUCCESS)
}

/// `forecast`: the disasters the mission sets off in its first marks, replayed with nobody playing.
fn forecast_cli(dll: &Path, args: &Args) -> Result<()> {
    let report = inspect::inspect(dll, None)?;
    let desc = report.desc.as_ref().with_context(|| format!("{} isn't a mission: {}", dll.display(), report.not_a_mission.as_deref().unwrap_or("?")))?;
    let mut start = setup::StartOptions::default_for(desc.mission_type, desc.num_players);
    start.difficulty = args.difficulty.unwrap_or(start.difficulty);
    start.players = args.players.or(start.players);
    let marks = args.marks.unwrap_or(disasters::DEFAULT_MARKS);
    if args.messages {
        let r = messages::report(dll, start, marks)?;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&r)?);
            return Ok(());
        }
        println!("{}: {} message texts in the DLL ({} more made while it runs)", dll.display(), r.texts.len(), r.runtime);
        for t in &r.texts {
            let seen = t.first.and_then(|i| r.replay.messages.get(i)).map_or(String::new(), |m| format!("  [replay: {}, from {}]", m.when(), m.from));
            let calls = if t.calls > 1 { format!(" (x{})", t.calls) } else { String::new() };
            println!("  {}{calls}{seen}", t.text);
            print_posters(&t.from);
        }
        if r.runtime > 0 {
            println!("  ({} calls with a text made while it runs)", r.runtime);
            print_posters(&r.runtime_from);
        }
        let m = &r.replay;
        println!("Replay: {} messages in the first {} marks (reached mark {})", m.messages.len(), marks, m.reached / disasters::MARK);
        for msg in &m.messages {
            let at = msg.at.map_or(String::new(), |(x, y)| format!(" at {x}, {y}"));
            println!("  {:<9} to {:<9} {}  [sound {}{at}, from {}]", msg.when(), msg.to_text(), msg.text, msg.sound, msg.from);
        }
        if let Some(s) = &m.stopped {
            println!("  stopped: {s}");
        }
        return Ok(());
    }
    let f = disasters::forecast(dll, start, marks)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&f)?);
        return Ok(());
    }
    println!("{}: {} disasters in the first {} marks (reached mark {})", dll.display(), f.disasters.len(), marks, f.reached / disasters::MARK);
    for d in &f.disasters {
        println!("  {:<22} {:<20} at {:<22} {:<14} from {}", d.when(), d.name(), d.place(), d.detail, d.callback);
    }
    if let Some(s) = &f.stopped {
        println!("  stopped: {s}");
    }
    for w in &f.warnings {
        println!("  warning: {w}");
    }
    Ok(())
}

/// "in ReinforceE, called by #4 TimeTrigger: ..." lines under a message text.
fn print_posters(posters: &[messages::Poster]) {
    if posters.is_empty() {
        println!("      no exported function reaches it (unused code, or called through a pointer)");
    }
    for p in posters {
        let by = if p.by.is_empty() { "no trigger seen calling it".to_string() } else { p.by.join("; ") };
        println!("      in {}, called by {by}", p.function);
    }
}

/// `render`: the mission's map after its setup, with everything on it, as one PNG.
fn render(dll: &Path, out: &Path, game_dir: &Path, args: &Args) -> Result<()> {
    let report = inspect::inspect(dll, Some(game_dir))?;
    let desc = report.desc.as_ref().with_context(|| format!("{} isn't a mission: {}", dll.display(), report.not_a_mission.as_deref().unwrap_or("?")))?;
    let files = op2_formats::GameFiles::open(game_dir)?;
    let mut map = files.read_map(&desc.map_name).with_context(|| format!("map {}", desc.map_name))?;
    let tilesets = op2_formats::render::MapTilesets::load(&files, &map);
    let mut start = setup::StartOptions::default_for(desc.mission_type, desc.num_players);
    start.difficulty = args.difficulty.unwrap_or(start.difficulty);
    start.players = args.players.or(start.players);
    let setup = setup::run(dll, start)?;
    if let Some(g) = &setup.graphics {
        export::apply_graphics(&mut map, g);
    }
    let art = sprites::Art::load(&files).map_err(|e| eprintln!("warning: object art: {e}")).ok();
    export::save_map(out, &map, &tilesets, Some(&setup), art.as_ref(), &scene::Layers::default(), args.size)?;
    match args.size {
        Some(px) => println!("{}: {} (at most {px} px)", out.display(), setup.summary()),
        None => println!("{}: {} ({} px per tile)", out.display(), setup.summary(), export::scale_for(&map)),
    }
    Ok(())
}

/// `render` on a folder: every mission DLL in it to `OUTDIR/NAME.png` (`.jpg` with `--jpg`), as each starts in the game (multiplayer with
/// every slot on the map unless `--players`). DLLs that aren't missions are skipped; a mission whose setup fails is
/// reported and the rest still render. Fails only when one did.
fn render_folder(dir: &Path, out_dir: &Path, game_dir: &Path, args: &Args) -> Result<ExitCode> {
    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let (mut done, mut skipped, mut failed) = (0, 0, Vec::new());
    for dll in dlls_in(dir)? {
        let name = dll.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !inspect::inspect(&dll, Some(game_dir)).is_ok_and(|r| r.desc.is_some()) {
            skipped += 1;
            continue;
        }
        let png = out_dir.join(Path::new(&name).with_extension(if args.jpg { "jpg" } else { "png" }));
        // A mission's own code runs in the x86 sandbox: a panic there costs that mission, not the batch.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render(&dll, &png, game_dir, args)))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("panicked")));
        if let Err(e) = result {
            eprintln!("{name}: {e:#}");
            failed.push(name);
        } else {
            done += 1;
        }
    }
    let failures = if failed.is_empty() { String::new() } else { format!(": {}", failed.join(", ")) };
    println!("{done} rendered, {skipped} not missions skipped, {} failed{failures}", failed.len());
    Ok(if failed.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

/// `opm`: the mission's starting state as an `.opm` file.
fn export_opm(dll: &Path, out: &Path, args: &Args) -> Result<()> {
    let report = inspect::inspect(dll, None)?;
    let desc = report.desc.as_ref().with_context(|| format!("{} isn't a mission: {}", dll.display(), report.not_a_mission.as_deref().unwrap_or("?")))?;
    let mut start = setup::StartOptions::default_for(desc.mission_type, desc.num_players);
    start.difficulty = args.difficulty.unwrap_or(start.difficulty);
    start.players = args.players.or(start.players);
    let setup = setup::run(dll, start)?;
    for note in opm::save(out, desc, &setup)? {
        eprintln!("note: {note}");
    }
    println!("{}: {}", out.display(), setup.summary());
    Ok(())
}

/// A path as given, or a bare file name looked up in the game folder.
fn resolve(target: PathBuf, game_dir: Option<&Path>) -> Result<PathBuf> {
    if target.exists() {
        return Ok(target);
    }
    match game_dir {
        Some(d) if target.components().count() == 1 => op2_formats::find_in_dir(d, &target.to_string_lossy())
            .map_err(|_| anyhow::anyhow!("{} not found here or in {}", target.display(), d.display())),
        _ => bail!("{} not found", target.display()),
    }
}

/// A report, and with `--objects` the mission's setup.
#[derive(serde::Serialize)]
struct Inspected {
    /// What the DLL says without running it; its fields sit at the top level of the JSON.
    #[serde(flatten)]
    report: inspect::Report,
    /// With `--objects`: what the mission's InitProc created.
    #[serde(skip_serializing_if = "Option::is_none")]
    setup: Option<setup::Setup>,
    /// With `--objects`: the setup's ore beacons by ore and yield, magma vents, fumaroles and wrecks.
    #[serde(skip_serializing_if = "Option::is_none")]
    resources: Option<setup::Resources>,
}

/// One DLL's report, and its setup with `--objects`.
fn inspect_one(path: &Path, args: &Args, game_dir: Option<&Path>) -> Result<Inspected> {
    let report = inspect::inspect(path, game_dir)?;
    let setup = match (&report.desc, args.objects) {
        (Some(d), true) => {
            let mut start = setup::StartOptions::default_for(d.mission_type, d.num_players);
            start.difficulty = args.difficulty.unwrap_or(start.difficulty);
            start.players = args.players.or(start.players);
            Some(setup::run(path, start)?)
        }
        _ => None,
    };
    let resources = setup.as_ref().map(setup::Setup::resources);
    Ok(Inspected { report, setup, resources })
}

/// `inspect`: one DLL in full, or a folder one line per DLL; JSON with `--json`.
fn inspect_cli(target: &Path, game_dir: Option<&Path>, args: &Args) -> Result<ExitCode> {
    if target.is_dir() {
        let all = dlls_in(target)?.iter().map(|p| inspect_one(p, args, game_dir)).collect::<Result<Vec<_>>>()?;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&all)?);
        } else {
            for i in &all {
                match &i.setup {
                    Some(s) => {
                        let name = i.report.file.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                        println!("{name:<14} {}", s.summary());
                    }
                    None if !args.objects || !i.report.is_mission() => inspect::print_line(&i.report),
                    None => {}
                }
            }
            let missions = all.iter().filter(|i| i.report.is_mission()).count();
            println!("{missions} missions in {} DLLs", all.len());
        }
        return Ok(ExitCode::SUCCESS);
    }
    let i = inspect_one(target, args, game_dir)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&i)?);
    } else {
        inspect::print_full(&i.report);
        if let Some(s) = &i.setup {
            s.print();
        }
    }
    Ok(if i.report.is_mission() { ExitCode::SUCCESS } else { ExitCode::from(2) })
}

/// The `.dll` files directly in `dir`, sorted by name.
fn dlls_in(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")))
        .collect();
    out.sort_by_key(|p| p.file_name().map(|f| f.to_ascii_lowercase()));
    Ok(out)
}

/// `lua`: converts a mission (multiplayer, campaign, colony game, auto demo or tutorial) to OP2Lua's three files in
/// `out`. The stub is OP2Lua's prebuilt
/// `LuaMission.dll` (`--stub`, else `$OP2LUA_STUB`, else beside op2dll.exe).
fn lua_cli(dll: &Path, out: &Path, simple: bool, stub: Option<&Path>, game_dir: Option<&Path>) -> Result<()> {
    let stub = stub
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("OP2LUA_STUB").map(PathBuf::from))
        .or_else(|| std::env::current_exe().ok().and_then(|e| Some(e.parent()?.join("LuaMission.dll"))).filter(|p| p.exists()))
        .context("lua needs OP2Lua's stub DLL: --stub path/to/LuaMission.dll (in the OP2LuaSDK's bin folder)")?;
    let template = std::fs::read(&stub).with_context(|| format!("reading {}", stub.display()))?;
    let desc = inspect::inspect(dll, None)?.desc.with_context(|| format!("{} isn't a mission", dll.display()))?;
    let lua = luaexport::convert(dll, game_dir, simple, &mut |s| eprintln!("  {s}"))?;
    let bytes = luaexport::stub(&template, &lua.name, &desc.map_name, &desc.techtree_name, desc.mission_type, desc.num_players, desc.unit_mission, lua.ai_players)?;
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let write = |name: String, data: &[u8]| -> Result<()> {
        let path = out.join(name);
        std::fs::write(&path, data).with_context(|| format!("writing {}", path.display()))?;
        println!("wrote {}", path.display());
        Ok(())
    };
    write(format!("{}.dll", lua.base), &bytes)?;
    write(format!("{}.placement.lua", lua.base), lua.placement.as_bytes())?;
    write(format!("{}.lua", lua.base), lua.mission.as_bytes())?;
    for n in &lua.notes {
        println!("  not converted: {n}");
    }
    Ok(())
}
