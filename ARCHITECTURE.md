# Architecture

How OP2MissionReader (the `op2dll` program) is put together, for contributors and agents new to the code. It follows
the template at architecture.md. Keep it current when modules or data flow change.

## 1. Project structure

One crate, about 15,000 lines of Rust: a library (`op2dll`, no UI) and the `op2dll` program on top of it. Running
mission code, reading the game's files and simulating the World are op2-rs's crates; this crate turns their results
into reports, pictures, `.opm` files and OP2Lua missions.

```
op2missionreader/
├── Cargo.toml            dependencies (op2-rs crates pinned to a commit), build profiles
├── build.rs              embeds the icon in the Windows exe
├── assets/Tech_Planet.ico the app icon (exe and every window)
├── rust-toolchain.toml   Rust 1.99, the toolchain op2-rs pins
├── README.md             usage, keys, building
├── ARCHITECTURE.md       this file
├── CHANGELOG.md
├── LICENSE               GPL-3.0
└── src/
    ├── lib.rs            the library (crate `op2dll`, no UI): the modules marked [lib], for other programs such as
    │                     OP2Imager (../op2imager), which uses it with `default-features = false`
    ├── main.rs           the program (needs the `app` feature: eframe, rfd, iced-x86, windows-sys): window,
    │                     `inspect`, `render`, `opm`, `forecast`, `messages`, `calls`, `lua`, `disasm`; Windows console
    │
    │   reading a mission (no UI)
    ├── inspect.rs        [lib] is this DLL a mission? its descriptor (DescBlock), map size; no mission code runs
    ├── setup.rs          [lib] runs DllMain + InitProc in the x86 sandbox; collects the objects it created
    ├── triggers.rs       [lib] the triggers the setup made, described: objectives, what each watches, areas
    ├── groups.rs         [lib] the AI groups the setup made (from op2-sim): orders, units, wants, building plans
    ├── disasters.rs      disaster forecast: op2-missionhost replays the mission headless
    ├── messages.rs       message texts from the DLL's AddMessage call sites, the exported functions whose code reaches
    │                     each (iced-x86, following branches and calls) and their triggers, and a headless replay
    ├── background.rs     a window's replay run on a thread of its own, keyed by mission, start and length
    ├── calls.rs          `calls`: every game API call of the setup and a replay, optionally from a multiplayer lobby
    ├── luaexport.rs      `lua`: a multiplayer mission converted to OP2Lua from many setup runs (layouts, levels, beacons)
    ├── replay.rs         a multiplayer setup as a replay: `on_init` makes the original's draws and calls in its order
    ├── colony.rs         `lua` for a colony game, tutorial or campaign mission: placement and win at each difficulty
    ├── victory.rs        the converted mission's win: victory conditions and their triggers as OP2Lua trigger.* calls
    ├── timed.rs          the converted mission's disasters and timed events: trigger functions remade from many calls
    ├── ai.rs             the converted mission's computer player: its AI groups' setup and group-only trigger functions
    ├── disasm.rs         `disasm`: an exported function or address as x86, imports and pushed text named
    │
    │   drawing
    ├── scene.rs          [lib] what a picture shows: object layers, draw order, each object's art and anchor, colours
    ├── sprites.rs        [lib] object art: composes frames from the game's art files
    ├── export.rs         [lib] whole map + objects drawn on the CPU (the mission's tile edits applied), shrunk
    │                     (Lanczos), saved as PNG, JPEG or BMP
    ├── exact.rs          [lib] a harness state drawn pixel for pixel as the game's detail view draws it (15-bit
    │                     colour, layer clipping, lasers), so OP2Imager's image matches the harness's
    ├── mapview.rs        map viewport: tile atlas mesh, zoomed-out overview image, pan/zoom
    ├── objects.rs        the window's objects: art or coloured dots; trigger, group, disaster and message overlays
    ├── textures.rs       the window's object art as textures, one per look, made when first drawn
    │
    │   saving the mission
    ├── opm.rs            the starting state as an OP2MissionEditor / DotNetMissionSDK .opm (JSON)
    │
    │   window
    ├── app.rs            main window: menus, details panel, object list, keys, saving, screenshots
    ├── windows.rs        View menu windows: objectives and triggers, AI groups, disasters and timers, messages, research by name
    └── browser.rs        mission browser window: scans the game folder in the background
```

## 2. High-level system diagram

```
                 Outpost 2 install (GOG 1.2 + OPU)                       mission DLL
     maps (.map), tilesets, op2_art.prt, OP2_ART.BMP, Outpost2.exe           │
                │                                                           │
                ▼                                                           ▼
        op2-formats (readers)                            inspect.rs ── op2-formats PE parser
                │                                            │        + Mission::load (descriptor only)
                │                                            ▼
                │                                         Report ───────────┐
                │                                                           │
                │                       setup.rs ── op2-missionhost: Mission::load + start
                │                                   (op2-x86 sandbox runs DllMain, InitProc;
                │                                    game API calls land in its World / op2-sim)
                │                                            ▼
                │                                         Setup (objects, beacons, walls,
                │                                                players, tile graphics)
                ▼                                            ├──► opm.rs ──► .opm file
   mapview.rs (tiles) ◄── export::apply_graphics ◄───────────┤
   sprites.rs (art)  ◄── scene.rs (layers, plan) ◄───────────┘
                │
       ┌────────┴─────────┐
       ▼                  ▼
   app.rs window      export.rs image / main.rs `render`, OP2Imager
   browser.rs window

   Setup's World ──► triggers.rs, groups.rs ──────────────────► windows.rs (Objectives and triggers, AI groups)
   mission DLL ───► messages.rs (code scan, iced-x86) ─┐
   op2-missionhost replay (forecast / Play) ───────────┴► disasters.rs, messages.rs ──► windows.rs, on a
                                                           background.rs thread       (Disasters, Messages)
```

Data flows one way: a DLL becomes a `Report` (static facts), a `Setup` (starting state, with its triggers and AI
groups) and, when a window asks, a replay's results; the views only read those. Nothing is written back to the game
folder.

## 3. Core components

**Reading a mission (`inspect.rs`, `setup.rs`).** No UI types; used by both the window and the command line.

- `inspect::inspect(path, game_dir) -> Report`: parses the PE file and decides whether it is a mission: 32-bit x86,
  a DLL, an `InitProc` export, and a `DescBlock` or `MapName` export. If so, op2-missionhost maps it to read the
  descriptor (name, type, players, tech level, map, tech tree) and, given the game folder, the map's size. No mission
  code runs, so this is fast and safe; the mission browser calls it for every DLL it finds.
- `setup::run(path, StartOptions) -> Setup`: the mission's setup. op2-missionhost loads the DLL into op2-x86's
  sandbox and calls `DllMain` and `InitProc` with the chosen difficulty and player count. Its game API
  (`TethysGame::CreateUnit`, `CreateBeacon`, `CreateWallOrTube`...) builds a World backed by op2-sim. Afterwards the
  World's live units become `Object`s, op2-sim's beacons become `BeaconLook`s, and its terrain gives the tile graphics
  (walls and tubes change tiles). A setup that stops early keeps what it made and records why (`failed`), along with
  any API calls the host doesn't implement (`unimplemented`). `OP2_BUDGET` raises the sandbox's instruction limit.
  `Setup::summary` counts what it made by kind, and `Setup::resources` counts its ore beacons by ore and yield, its
  magma vents, fumaroles and wrecks (`inspect`'s `Resources:` line and the window's Objects summary).
- `setup::from_state` reads an OP2TestHarness state (`state_<tick>.json`: tiles and units at a tick, in file pixels)
  into `StateParts`, and `setup::from_parts` turns those into the same shapes, so a moment of a played game can be
  drawn like a setup; OP2Imager builds `StateParts` from a saved game too.
- `Object` is the program's own view of a unit: kind, type name, owner, tile, pixel, colour, weapon, facing and the
  type-specific `extra` fields; `describe()` and `details()` turn it into text.
- `PlayerStart` is each player's starting state (people, ore, food, morale, research), from op2-missionhost's
  `PlayerRec`.

**What the mission's code does next (`triggers.rs`, `groups.rs`, `disasters.rs`, `messages.rs`, `background.rs`).**

- `triggers::collect` reads the triggers the setup made from the World: kind, what each waits on in words (tiles in
  file coordinates, research by name from the tech tree, times in marks), whether it repeats, the function it calls,
  and the victory and failure conditions that use it.
- `groups::collect` reads the AI groups op2-sim holds: fight, mining and building groups with their orders, units,
  wanted units and building plans.
- `disasters::forecast` runs op2-missionhost's `forecast::predict`: the mission replayed with nobody playing for 100 to
  2000 marks, and the disasters its code sets off, with when, where and which function.
- `messages::texts` finds every `AddMessage` call in the DLL without running it (the three call forms through the
  import table, and the text pushed just before), then follows the code of every exported function with iced-x86
  (both sides of branches, switch tables, called helpers) to learn which exports reach each call. `messages::replay`
  replays the mission and records the messages it posts and the triggers it makes later; `report` joins the two, so
  each text names the function that posts it and the setup, `AIProc` or triggers that call that function.
- Replays take seconds, so `background::Job` runs them on a thread of their own, keyed by the mission, start options
  and length; a window says it is replaying until the result arrives, and runs again when the key changes.

**Drawing (`scene.rs`, `sprites.rs`, `export.rs`, `exact.rs` in the library; `mapview.rs`, `objects.rs`,
`textures.rs` in the window).**

- `MapView` owns the map and its tilesets. It packs each tile mapping's first frame into one texture atlas and draws
  the visible tiles as a single mesh. Below about 6 screen pixels per tile it draws a pre-composed overview image
  instead, so a whole 512×256 map stays cheap. `Transform` converts map pixels to screen points for the overlays.
- `scene::plan` orders what to draw (resources first, then by pixel row) and filters it by `Layers` (the art/dots
  switch, the kind toggles, hidden players); it is shared by the window and the exported image, so both show the
  same. `objects::draw` draws art at 8 or more screen pixels per tile and dots otherwise, and reports which object is
  under the pointer.
- `sprites::Art` composes an object's frame with op2-formats (chassis and turret, tinted for the owner, with the
  animation tables read from the player's Outpost2.exe). `Look` is the cache key, and each distinct look becomes one
  egui texture the first time it is drawn.
- `export::draw` draws the same picture on the CPU at up to 32 px per tile (no side longer than 8192 px), using
  `render::compose` for the tiles. `export::fit` shrinks it with a Lanczos filter when a smaller size is asked for
  (drawing at full size first keeps small units readable), and `export::save` writes it by the file's extension:
  PNG, JPEG (quality 85) or BMP. `export::save_map` does all three; the window never holds the whole map at full size.
- `exact.rs` is a second, stricter drawer for OP2Imager's harness-state images: it reproduces the game's own detail
  view pixel for pixel (15-bit colour, shadows, each sprite layer's clipping, laser and microwave lines, the view
  pieces the harness stitches), so the result matches the harness's own screenshot byte for byte.

**Saving the mission (`opm.rs`).** `opm::build(desc, setup)` turns the descriptor and the `Setup` into the SDK's
`MissionRoot`; `opm::save` writes it out. The structs mirror the SDK's DataContract classes
(`DotNetMissionSDK/MissionReader/Json`), with members declared alphabetically so the output looks like the editor's:
tabs, every member present.
- **Names:** enums are written by name, from the SDK's `map_id`, `TruckCargo`, direction, colour, morale and marker
  tables.
- **Positions:** file tile + 1 on both axes, the inverse of the SDK's `GetMapCoordinates`.
- **Mines:** a mine absorbs the beacon under it, since the SDK creates a mine's beacon itself.
- **Player state:** from `setup::PlayerStart` (op2-missionhost's `PlayerRec`).
- **Daylight, morale, music:** from the setup's recorded options and music list.
- **Left out:** what the format can't hold comes back as notes for the user.

A .NET check that reads exports with the SDK's own `MissionReader` is described under section 8.

**Window (`app.rs`, `browser.rs`).** eframe/egui 0.37, immediate mode: every frame redraws from `App`'s state.

- `App` holds the game folder, the open mission (`Loaded`: report, setup, map view, selection, layers) and the
  browser. User input becomes `Action`s (open, step, toggle a layer, save...), applied after the frame is drawn.
- The View menu windows (`windows.rs`) are native windows too: Objectives and triggers, AI groups, Disasters and
  timers, Messages and Research. Picking a row in one sends an `Action` that outlines or marks it on the map
  (`objects::draw_areas` and the other overlays) and centres the map there.
- The mission browser is a second native window (`show_viewport_immediate`). `Browser::scan` walks the game folder up
  to 5 levels deep on a background thread and sends `Entry`s over a channel, so the list fills in as it is read
  (about 25 seconds for an OPU install).
- File > Save whole map calls `export::save_map` with the size chosen under File > Whole map size (full, or 4096 to
  512 px on the longer side); a `.jpg` name saves a JPEG. The window title and `--help` show the crate's version.
- `--screenshot` (`ScreenshotJob`) waits for the map and the browser to finish loading, then saves the window and
  exits. It is used to check the window headlessly.

**Command line (`main.rs`).** `inspect` prints a `Report` (and, with `--objects`, the `Setup` and its resource counts)
as text or JSON, for one DLL or a whole folder. `render` runs the setup and calls `export::save_map`, at full size or
`--size` pixels on the longer side, as PNG or JPEG; on a folder it renders every mission DLL to `OUTDIR/NAME.png`
(`.jpg` with `--jpg`), skipping DLLs that aren't missions and going on past one whose setup fails (a panic in the
sandbox is caught per mission). `opm` calls `opm::save`. `forecast` and
`messages` run the replays above (600 marks unless `--marks`) and print what the windows show. `calls` prints
op2-missionhost's call log of the setup (and of `--marks` more), started from a lobby's `start_info.json` with
`--start`, and with `--call` the calls of mission exports run after it. `lua` (`luaexport::convert`) runs the setup
over player counts, lobby choices and seeds, sorts what it made into base layouts by area, levels, colonies and
initial vehicles, and writes the Lua. Without a computer player, `replay::convert` writes `on_init` instead as a
replay of the original's setup: the runs as tokens (runs of draws, calls), aligned on what they share, an `if` on the
draw or lobby setting that decides each choice, each player's base on its own, draws as plain draws, `pick`,
`draw_avoid` or a loop per initial vehicle; the placement then makes nothing. `victory::convert` turns the recorded victory conditions and their triggers into
`trigger.*` and `mission.victory` calls (Land Rush's `Base<N>` functions are measured by calling them);
`timed::convert` calls each other trigger function 600 times (`calls::repeat`), reads the game's random draws as
inputs and every other call as outputs, and writes the rules that link them: each number a constant, a draw times a
number plus a number, two draws added, or a table indexed by a draw; an `if` where an earlier draw decides what comes;
a rectangle test redone until it passes as a `repeat` loop. A trigger whose function isn't converted is still made,
calling an empty function, so the game draws its timer as the original does. With a computer player, `ai::setup` writes its calls of
the setup (on the player after the humans, its groups and units) one for one, reading each `UnitBlock`'s records from
the DLL mapped but not started; a trigger function that calls group methods is called 40 times instead and written by
`ai::function`, as one body or as numbered steps. A colony game, tutorial or campaign mission goes to
`colony::convert`: its placement and win at each difficulty, the Lua branching on it where they differ. `disasm`
(`disasm.rs`) prints an export or address as x86 with imports and pushed text named; `--block --shipped` prints a
`UnitBlock`'s table as the DLL ships it, before the game sorts it. Release builds
on Windows use the GUI
subsystem, so they attach to the parent console for these modes, and show errors in a message box when started
from Explorer.

## 4. Data stores

There is no database and nothing is cached on disk. The program reads:

- **The game folder:** found from `--game-dir`, `$OP2_DIR`, the folder op2-rs saved in its settings (op2-settings),
  or the usual install places (op2-formats' `install::use_for_tool`). It is only read.
- **Mission DLLs:** read whole into memory, and mapped into the sandbox only for the setup.

In memory, the per-look textures (`sprites::Art`) and the tile atlas live as long as the open mission, as do the
last forecast and messages replay.

## 5. External integrations

- **op2-rs** (gitlab.com/bufke/op2-rs, GPL-3.0-or-later). Its crates come in as git dependencies pinned to one
  commit in `Cargo.toml`, because their APIs change. README.md shows how to build against a local clone instead.
  - `op2-formats`: PE parsing, `.map`, tilesets, art and the unit art tables read from Outpost2.exe (`exe::GameExe`),
    tech trees, the install lookup and the map overview renderer.
  - `op2-x86`: the x86 interpreter sandbox.
  - `op2-missionhost`: loading mission DLLs and the game API they call; `Play` and `forecast` replay a mission.
  - `op2-sim`: the simulation world behind that API (beacons, terrain, looks, triggers, AI groups).
  - `op2-settings`: where op2-rs saved the game folder.
- **Other crates:** eframe/egui (windows, drawing), rfd (file dialogs and message boxes), image (PNG, JPEG and BMP
  out, and the Lanczos resize), serde and
  serde_json (`--json`), anyhow (errors), iced-x86 (decoding mission code for the messages window; op2-rs already
  uses it), and windows-sys (`AttachConsole` on Windows).

- **The .opm format** (OP2MissionEditor and OP2DotNetMissionSDK): only written. Its model is the SDK's
  `DotNetMissionReader` classes, and OPM_FORMAT.md in op2opmtools describes it.

- **OP2Imager** (`../op2imager`, github.com/leviathan400/OP2Imager) uses the library with `default-features = false`, so it brings no
  windowing crates; it is the batch image tool (maps, mission DLLs and harness states), while `op2dll render` stays a
  quick way to picture one mission or a folder of them.
- **OP2TestHarness** states (`state_<tick>.json`) are read by `setup::from_state` and drawn by `exact.rs`.

Fixes to how missions run belong in op2-rs, upstream: op2-missionhost, by a merge request from a fork.

## 6. Deployment and infrastructure

It is a desktop program with no services. It is built locally with `cargo build --release`; the output is
`target/release/op2dll.exe`. The source is on GitHub (leviathan400/OP2MissionReader); its CI
(`.github/workflows/build.yml`) runs clippy and the tests and builds Windows and Linux binaries on every push. Both
build and pass; the program has been run on Windows only. Nothing used is Windows-only, so macOS should build too,
untried.

## 7. Security considerations

A mission DLL is untrusted code, and it never runs natively: `InitProc` runs in op2-x86's interpreter, and its only
way out is the game API op2-missionhost provides. An instruction budget stops runaway setups, and a folder `render`
catches a panic per mission so one bad DLL doesn't stop the rest. The only files written are the images, `.opm`
files and Lua missions the user asks for.

## 8. Development and testing

- Rust 1.99 (`rust-toolchain.toml`). Dependencies are optimized even in dev builds, because the sandbox is slow
  unoptimized.
- Keep `cargo clippy --all-targets` clean, and every item documented (doc comments on every struct, enum, function
  and constant). The only unit test checks that the icon decodes. Checks so far are by hand:
  - `op2dll inspect --objects` over the game folder (every mission DLL is listed);
  - `op2dll render` on the game folder: every mission's start drawn (106 missions, none failing, on GOG + OPU 1.4.1);
  - `op2dll messages` and `forecast` over every mission DLL, read against the missions' known behaviour;
  - `--screenshot` and `render` PNGs, compared against the game;
  - `.opm` exports read back with the SDK's own reader. A throwaway .NET console project compiles
    `op2dotnetmissionsdk/DotNetMissionSDK/MissionReader/**/*.cs` and calls `MissionReader.GetMissionData` on each file.
    On 2026-10-30, exports of campaign, tutorial, colony and every multiplayer type all read back, as did the
    editor-made `opu_12.opm`.
- Game data is never committed (`target`, `*.png` and `*.map` are ignored).

## 9. Future considerations

- `mr4Yuko.dll` (OPU YukonTrail) runs out of steps in its setup (since op2-rs 877421c; before, it stopped at
  `kernel32!LCMapStringEx`). The fix belongs in op2-rs, through a merge request.
- A mission whose helper DLL isn't beside it (OPU's DangerZone imports `op2extra.dll`) runs with those imports as
  stubs, and says so.
- Messages posted through a function pointer, or with text built at run time (`%s` formats), can't be tied to a
  function by the code scan; the replay still sees them if they come.
- Beacons are drawn as if surveyed, a choice made here. The game hides rare ore until it is researched and shows
  every unsurveyed beacon alike.
- The program could be renamed from `op2dll` to op2missionreader.
- No automated tests yet: a test that runs `inspect` on a mission found through `$OP2_DIR`, skipping when there is no
  install, would catch regressions when the op2-rs pin moves.

## 10. Project identification

- **Name:** OP2MissionReader (crate and binary `op2dll`), version 0.4.0.
- **Repository:** github.com/leviathan400/OP2MissionReader.
- **Contact:** Leviathan (leviathan400).
- **Last updated:** 2026-11-20.

## 11. Glossary

- **Mission DLL:** an Outpost 2 level. A 32-bit Windows DLL that exports `InitProc`, `AIProc` and a descriptor.
- **DescBlock:** the exported descriptor: mission type, player count, tech level, and whether units are shown.
  `LevelDesc`, `MapName` and `TechtreeName` are exported alongside it.
- **InitProc:** the mission's setup function. The game calls it once to place the starting units, beacons and walls.
- **OPU:** Outpost Universe's 1.4.x update of the game. Its extra missions are under `OPU/maps`.
- **Beacon:** a mining beacon, magma vent or fumarole. Ore beacons are common or rare, with a yield of 1 to 3 bars.
- **.opm:** the JSON mission file of OP2MissionEditor and the OP2DotNetMissionSDK, which loads it at run time.
- **Look:** an animation id, frame and tint: what a sprite is composed from, and the art cache's key.
- **Harness state:** OP2TestHarness's `state_<tick>.json`, the map's tiles and every unit at one tick of a run.
- **Tick, mark:** game time. A mark is 100 ticks; the game's clock and trigger times are shown in marks.
- **Trigger:** a condition the mission registers (a time, a count, a unit reaching a place, research done...) and the
  exported function the game calls when it fires. Victory and failure conditions are built on triggers.
- **AddMessage:** the game API call that posts a line to the message log, optionally with a place and a sound.
- **File tile vs. internal tile:** op2-missionhost's World offsets x by 32 on non-wrapping maps (`pad`). `setup.rs`
  removes the offset, so everything here uses the map file's tile coordinates.
