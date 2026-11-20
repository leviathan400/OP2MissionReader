# Changelog

Notable changes to OP2MissionReader (binary `op2dll`). The format follows [Keep a Changelog](https://keepachangelog.com/).

## [0.4.0] - 2026-11-20

### Added
- `setup::from_parts` draws a running game's state from plain parts (`StateParts`: tiles, players, objects), so a
  saved game (OP2Imager) and a harness state (`from_state`, which now reads into those parts) take one path.
  `setup::type_name` gives a map id's code name, `setup::is_weapon_fire` tells a weapon's fire from a unit.

### Changed
- The project is named OP2MissionReader: the window title and `--help` say so. The program is still `op2dll`.
- op2-formats 0.6 (crates.io), was 0.4. op2-rs's crates still use 0.4, so both are built.

## [0.3.0] - 2026-11-20

### Added
- `--help` and the window's title show the version.
- Doc comments on every struct, enum, function and constant, and on the fields and steps whose meaning isn't plain
  from the code (units, coordinates, encodings, magic numbers).
- The window saves the whole map as PNG or JPEG, at full size or at 4096, 2048, 1024 or 512 px on its longer side
  ("Whole map size").
- `inspect --objects` counts the setup's resources: ore beacons by ore (common, rare) and yield (3-, 2-, 1-bar, random),
  magma vents, fumaroles and wrecks (a `Resources:` line, `resources` in the JSON); the window shows the same line
  under its Objects summary.
- `exact`: a whole-map image drawn as the game's detail view draws it, from what a game state says the game drew
  (OP2TestHarness's `views`): its 15-bit colours (sprites truncated to 5 bits a channel, shadows darkening to 3/4),
  animated tiles at the state's frame, sprites layer by layer with their clip rectangles, and laser and microwave
  lines clipped and stepped as the game does, then shrunk by the harness's block average. The same pixels as the
  harness's images. `Art::sprites` gives it the loaded art.
- `setup::from_state`: an OP2TestHarness game state (`state_<tick>.json`) as a `Setup` the exporter draws: its tiles as
  the graphics, its units, structures, beacons (ore and yield), vents and fumaroles as objects. OP2Imager draws states
  with it.
- A library (`src/lib.rs`, crate `op2dll`) with what other programs need to draw a mission's start: `inspect`,
  `setup`, `triggers`, `groups`, `sprites` (art), `scene` (layers, draw order, colours) and `export` (draw, resize,
  save as PNG, JPEG or BMP). Built with `default-features = false` it needs no windowing libraries; the op2dll program
  needs the `app` feature (on by default). OP2Imager (`../op2imager`) is built on it.
- `render` saves a `.bmp` output as a 24-bit BMP.
- `op2dll render FOLDER OUTDIR` renders every mission DLL in a folder to `OUTDIR/NAME.png` as it starts (a
  multiplayer map with all its player slots), skipping DLLs that aren't missions and going on past a mission whose
  setup fails.
- `op2dll render --size PX` shrinks the image so its longer side is at most PX pixels (drawn at full size first, then
  resized with Lanczos), and a `.jpg` output (`--jpg` on a folder) is saved as a JPEG (quality 85).
- `op2dll lua` replays a multiplayer mission's setup (`src/replay.rs`): it runs the setup over many seeds and lobbies
  (every player count, colonies, resource level, initial vehicles, day and night, morale, disasters), and `on_init`
  makes the original's draws of the game's random numbers in its order and, from them and the lobby, the same units,
  beacons, walls, resources and research in the same order, so a game is the original's. Draws come as plain draws,
  `pick(range, k, at)` (k different values, redrawing one it has), `draw_avoid(range, taken)` (redrawn while it equals
  an earlier draw) and a loop once per initial vehicle; a choice branches on the draw or lobby setting that decides
  it, a value is a constant, a line or a table in one or two of them. Each player count on its own, or all together,
  with the initial vehicles as a loop or written out: the shortest that works, up to 8 MB; else the layout model as
  before, with the reason at the end of the Lua. A branch no sampled game took logs `replay: ... a way no sampled
  game took`. 55 of the 59 stock maps replay to match their originals every tick (OP2TestHarness, 2 to 6 players); a mission with a
  computer player isn't replayed yet. op2-missionhost gives every player the host's resource level, so lobbies of
  mixed levels aren't sampled.
- The replay's draws: a draw redrawn while it's one of any earlier draws of its range, or one plus a fixed offset
  (Wishbone, Flood Plain); a pick then plain draws of the same range. A setup is cut where a stretch made once per player starts, so each base follows its own draws; a
  block made as many times as there are players isn't taken for the initial vehicles' loop. `draw_avoid` logs instead
  of drawing for ever when every value is taken. `OP2_REPLAY_DEBUG=1` prints the alignment and each attempt's size;
  `OP2_REPLAY_MAX_KB` sets the largest replay kept.
- A multiplayer mission's triggers whose function isn't converted are made, calling an empty function, as in the
  colony games (their timer's draw is the original's). A multiplayer trigger function's reads of the game after its
  last action that led to nothing in any sampled call are left out, with a comment (Biohazard's `Quake`).
- Export to `.opm` (OP2MissionEditor / OP2DotNetMissionSDK JSON): File > Export as .opm (Ctrl+E) and the `opm`
  command. It writes the starting state from the setup run shown: mission details, players' colonies, colours,
  resources, research and camera, units, walls and tubes, beacons, markers, wreckage, daylight and music. The export
  says what it had to leave out: computer players' AI, factory bays, units owned by no player, and objects such as
  animations.
- What units hold: a Cargo Truck's load and amount, a Structure Factory's or Spaceport's bays (kit and weapon), and a
  Garage's vehicles by bay. The object list marks them (`[food, 1000]`, `[3 in bays]`, `[in a garage]`); vehicles
  parked in a Garage aren't drawn on the map, and the .opm export leaves them out with a note.
- Players table: each player's workers, scientists, kids, common and rare ore, food, solar satellites, morale and
  research done at the start. `inspect --objects` prints them too, and its JSON has them as `start`.
- View > Objectives and triggers: a window listing the mission's victory and failure conditions with what each waits
  on, and every trigger the setup made (what it watches, whether it repeats, the function it calls). Clicking one
  with an area outlines it on the map and centres there; View > Trigger areas on the map outlines them all (off by default).
  `inspect --objects` lists the triggers too.
- View > Research: a window listing each player's research done at the start by name, from the mission's tech tree
  (next to the DLL, else the game's).
- View > AI groups: a window listing the computer players' fight, mining and building groups the setup made: what
  each is told (objective, target, patrol route, mine and smelter, the groups it reinforces), its units, the units it
  wants, and a building group's plans (the structures, mines, walls and tubes it rebuilds). Picking a group outlines
  its area, route and units on the map and draws its plans as dashed outlines, green where nothing stands yet; a
  checkbox shows every building group's plans at once. `inspect --objects` lists the groups too.
- View > Messages: every message text the mission can post ("Reinforcements have arrived!"), read from the DLL's
  `AddMessage` calls without running it, then the messages a replay with nobody playing posts: when, to whom, the
  mission function that posted it, and the place it points at (marked on the map). `op2dll messages` prints them.
  Each text also names the exported function that posts it (followed through the code, helpers included) and what
  calls that function: the setup, `AIProc`, or the triggers the setup or the replay made, with what they wait on.
- `op2dll calls`: every game API call a mission makes, in order, with arguments and results: its setup and, with
  `--marks`, a replay. `--start` starts a multiplayer mission from a lobby's `start_info.json` (seed, colonies,
  resource levels, day and night, disasters, Morale Steady, initial vehicles), so runs can be compared to learn what
  the mission does with each choice. Used to remake `ml2_31.dll` (Tug-O-War) in OP2Lua. `--call EXPORT` then calls
  one of the mission's exported functions, as if its trigger had fired.
- `op2dll lua`: a multiplayer mission converted to an OP2Lua mission (placement.lua, the mission's Lua and a stub DLL),
  from what its setup does over hundreds of runs: base layouts and who gets them, units by resource level and colony,
  initial vehicles, beacons, tubes, day and night, morale, and the win of each multiplayer type (Last One Standing,
  Space Race, Resource Race, Midas, Land Rush). What isn't converted is listed at the end of the Lua.
- `op2dll lua` converts disasters and other timed events: each function a trigger calls is run 600 times and remade
  as Lua with the same draws of the game's random numbers (meteors, earthquakes, eruptions, storms and vortexes; a
  tile rolled again while it's inside a rectangle; the next time trigger made and the old one ended). Triggers the
  setup makes only when the lobby allows disasters go under `game.disasters_enabled()`.
- `op2dll lua` converts a mission's computer player (`DescBlockEx` asks for one): its setup calls on the game's AI
  groups (building, mining and fight groups, their records, target counts and reinforcements), its unit tables read
  from the DLL as it ships, and the trigger functions that only work on its groups (Peacekeeper's and La Corrida's
  "Expand" and "TimeUpgrade"), as one body or, when calls go through a list, as steps. The stub asks the game for
  the computer player too. Uses OP2Lua's `ai.*` calls.
- `op2dll lua --simple`: a simple version of the mission (`<dll>Simple`, "(Lua, simple)"). Each area gets one base,
  the same every game: its first layout whose structures all stand on passable ground (footprints from the game's
  structure table), written as flat tables of structures, vehicles, tubes and beacons. Every resource level gets
  High's units; beacons the original rolls are one fixed pick. Who gets which area, the win, disasters, day and night,
  morale and the computer player are as in the faithful conversion.
- `op2dll lua` converts Land Rush starts. Each player's column of vehicles runs one of 8 ways from its
  Robo-Surveyor, rolled each game, with the lobby's initial vehicles on 4 spots that turn with it: the layout lists
  each way's tiles, facing and spots (`columns`), and the Lua rolls one. The simple version takes the first way
  whose units all stand on ground a vehicle can take. When no way does, it moves the first way's column to the
  nearest place where its units and spots all do, with a clear tile around each unit, and notes the move (Axen's
  Home SW: 7 east, 4 south).
- `op2dll lua` converts lava: where it may flow (the setup's `SetLavaPossible` tiles, as `LAVA_POSSIBLE` areas set in
  `on_init`, clipped to the map), and trigger functions that change tiles (`SetTile`, written `game.set_tile`) or mark
  lava tiles. A function that goes quiet for good after the call that acts (Close Encounter's `Lava`) is measured
  over many games, each with the random sequence moved along, and gets a `done` flag. A trigger made once for each
  player alike becomes a loop over the players. Draws whose branches take the values in bands are written as
  `r < n`, so values the runs didn't draw go the way of their neighbours; an empty `else` is left out.
- `op2dll lua` converts count triggers a function makes (`CreateCountTrigger`), and triggers on one player by
  number (`player = n`). A function that only reads every player's kids, workers and scientists and then acts is
  measured by setting the colonies' population and searching for the total that makes it act, and written as a
  population test (Biohazard's `LavaCheck`: 200).
- `op2dll lua` converts a colony game's starting placement (CEP1, CES1, CPP1, CPS1, CPS2). The setup runs at
  Easy, Normal and Hard over many seeds. What every difficulty makes alike goes in placement.lua with the players'
  colonies and colours. The rest goes in the Lua by difficulty:
  - units, tubes and beacons only that difficulty makes;
  - each player's colonists, stores, research, Structure Factory kits and camera;
  - morale.
  Rolled beacons are written as spots picked from a list, or anywhere in a rectangle. A colony whose units all move
  together from game to game is a kit on a rolled start. Units placed with `MAP_RECT::RandPt` are made anywhere in
  their rectangle, and a `CreateUnitBlock`'s units are read off the map (a mine as the Common Ore Mine the game turns
  into a Rare Ore Mine or a Magma Well: `CreateUnit` of a Rare Ore Mine crashes the game). The win is converted as for
  multiplayer missions, with a trigger on one player named by number. Other triggers, and the AI groups the setup
  makes, are listed as not converted yet; so is `AIProc` when a replay sees it make game calls.
- `op2dll lua` converts the tutorials (t01 to t14) the same way: their start (the same at every difficulty, written
  once) and their objectives.
- `op2dll lua` converts victory and failure conditions on time triggers.
- `op2dll lua` converts the auto demos (ademo1 to ademo3) as it does the colony games: their start and their end.
  Their AI groups and trigger functions are finished by hand, and all three then play tick for tick as the originals
  in OP2TestHarness.
- Colony games, tutorials and auto demos list each player's units in the order the setup made them, a unit block's
  among the rest (the game numbers units in that order and moves them in it each tick), and keep a vehicle's lights
  off where the setup left them off (`lights = false`: `CreateUnit` makes them off, OP2Lua's default is on).
- `op2dll lua` keeps a Cargo Truck's starting load (`cargo`, `amount`), read from the units on the map after the
  setup, spacecraft, wreckage and gene banks included.
- Colony games, tutorials and auto demos: a beacon the setup makes between its units gets `after = n` in
  placement.lua (OP2Lua makes it once n units are made), so the units keep the original's object ids (Tutorials 3
  and 9). Their time-trigger disasters are converted as the multiplayer maps' are (Tutorial 3's earthquake), and so
  are the setup's own map edits (`SetTile`, `SetCellType`, `InitialSetTile`), each number fitted to the random draw
  before it (Tutorial 5's 18 tiles).
- The Scorpion's weapon is written as `EnergyCannon` (it had op2-missionhost's `SCORPION_WEAPON`).
- `op2dll disasm DLL EXPORT|0xADDRESS`: a mission's code as x86 instructions, with the imports it calls and the text
  it pushes named, to an export's last `ret` or for `--count N` instructions. `--dwords N` prints N dwords of the
  mission's memory from the address after its setup instead (the tables and rectangles its code reads, which its
  static constructors may fill in).
- `op2dll calls --repeat` runs a single-player mission's functions at its difficulty, without a lobby.
- Colony games: a trigger whose function isn't converted is still made, calling an empty function, so the game draws
  a random trigger's time as the original does and numbers later triggers the same; the list at the end of the Lua
  says which. A function that asks the player's difficulty is converted at each difficulty, the Lua picking the body
  by `players[1].difficulty`. Trigger functions may set tiles as the map loads and cell types (`InitialSetTile`,
  `SetCellType`). The win's triggers and the timed ones are made in the setup's order.
- Converted functions write runs of tiles marked where lava may flow as rectangles, not one call per tile.
- Colony games: when a difficulty's own units come among those alike at every difficulty (CEP1's Normal), each
  difficulty makes its units and beacons as one list in the order the setup made them (`made`), rolled beacons at
  their place. Rolled beacon spots are drawn as an index into the DLL's table (learned from the draws), again on a
  repeat; a beacon rolled in a box as two draws added to a base.
- Colony games: beacons keep the original's place among the units when a difficulty adds units (CPP1's Easy), when
  every unit is a difficulty's own (CPS1: `late_beacons`, made after them), and when a difficulty's own beacon comes
  between the common ones (CPP1's 181,12).
- Colony games with a computer player (CES1, CPS2): its base is converted as in the multiplayer missions, made where
  the setup makes its first group. Its exported unit tables are made with `create_units` (CES1's base); a unit put on
  a tile drawn in a rectangle draws there with `game.rand_point`; a unit the setup puts off the map keeps its raw
  tile; a fight group's patrol route is read from the DLL (`set_patrol`); `SetColorNumber` is left to the game. Its
  functions that make time triggers are converted, with the functions those triggers call; a function whose first
  call ends its own trigger is converted from that call. Units the computer player makes are left out of the colony's
  lists.
- Colony games: beacons the setup makes between its triggers are made there, after the trigger before them (CES1's
  six, which must come before ExpandBase1_A's trigger draws its time); a beacon under a mine of the placement stays a
  difficulty's own beacon, so the mine has it (CPS2).
- Colony games: a function that asks the difficulty and works on the computer player's groups is converted at each
  difficulty too (CES1's ExpandBase1_A).
- Campaign missions (`e01`-`e12`, `p01`-`p12`), with the colony games' conversion:
  - Their triggers: escape, kit, point, rect and evacuation triggers (`trigger.escape/kit/point/rect/evac`, tiles
    as the mission passes them), a Cargo Truck count by its truck cargo (a number), and the objectives whose
    triggers call a function (the function converted with the timed ones, or made empty).
  - In trigger functions: messages (`TethysGame::AddMessage` as `game.message_at`; a function that goes quiet after
    one may post it once) and the Blight (`SetVirusUL`, `SetMicrobeSpreadSpeed`).
  - The computer player: the player whose groups the setup makes first (player 3 in Eden 4); one with no groups in
    the setup still is, so the trigger functions that make its waves convert (Eden 7); `AllyWith`, a group's target
    counts from a unit table (`set_targ_counts`), a unit looked up in a group (`GetFirstOfType`), calls on units it
    makes and names later (`SetOppFiredUpon`, `SetDamage`, `DoIdle`: kept as locals); temporary `_Player` objects
    ignored.
  - The setup's wreckage (`CreateWreck` as `game.create_wreckage`), made where the setup makes it, each wreck on a
    fixed tile or on a tile drawn from a table learned from the games (Eden 9's).
  - The setup's markers (`PlaceMarker` as `game.create_marker`: the convoys' goals in Eden 6, 9 and 11 and Plymouth
    6, 9 and 10), made after the trigger or group the setup makes before each, as markers are objects.
  - The area the setup gives a player's default group (`GetDefaultGroup`, then `FightGroup::SetRect`, as
    `player:default_group():set_rect`: Eden 7, 8, 10 and 12), where a unit a Spider reprograms goes; another player's
    default group made one of the computer player's groups (`SetDefaultGroup`: Eden 4's and Plymouth 4's ally's).
  - A ConVec the setup loads after making it (`Unit::SetCargo`) keeps its kit and the kit's weapon: `kit_topping` in
    the placement, `topping` in a difficulty's lists (Eden 7's and Plymouth 7's Guard Post kits).
  - Tubes and walls the setup makes after the computer player's first group are made after `setup_ai`, where the
    setup makes them (Plymouth 10's around the guard posts, which made first would push the posts aside); those on
    tiles the setup's map edits changed before them are made after the edits (Plymouth 12's seven tubes).
  - Each difficulty's own setup triggers: the objectives and timed triggers are converted for Easy, Normal and Hard,
    and where they differ `on_init` makes them in an `if` on the difficulty (Plymouth 2's lava and quakes, Plymouth
    7's stores). A difficulty's trigger kept where Normal's is (the same dword of the mission's data after the setup:
    the handles its functions read, Plymouth 2's kits in ConVecs on Hard), else one like it, keeps Normal's name
    (`t27`); one only it makes has its own (`t28h`), declared before the `if`. The functions only another
    difficulty's triggers call are converted too. A name already declared earlier in `on_init` isn't declared again.
  - Each difficulty's own computer player: `setup_ai` makes Easy's, Normal's or Hard's base and groups, where they
    differ in an `if` on the difficulty (Eden 6's and Plymouth 10's blocks, Eden 11's labs, Plymouth 10's guard
    targets). Another difficulty's groups take the names of Normal's they're set up most like (Plymouth 6's Easy
    numbers them differently), and its units Normal's names on the same tile; a group only it makes is named for it
    (`ai_fight6_hard`). Its unit blocks are numbered after Normal's and the functions'.
  - A vehicle the setup parks in a Garage (`Unit::PutInGarage`) is parked there: `garage = { bay =, at = }` in a
    difficulty's lists, made with OP2Lua's `unit:put_in_garage` (Eden 1's and Plymouth 1's on Hard).
  - Each difficulty's day and night: `set_daylight_everywhere` and `set_daylight_moves` take the difficulties that
    set them on where they differ (Easy's daylight in Eden 11 and Plymouth 10, Hard's night in Eden 9, Plymouth 9
    and 12).
- Colony games and campaign missions: units alike at every difficulty that the setup makes after the computer
  player's base (Eden 4's human vehicles) are made right after `setup_ai`, not by the placement, so they keep the
  original's object numbers; a Structure Factory of the placement gets its bays' kits from the difficulty (Eden 1);
  a mining group set up from units the setup made finds them on their tiles (`unit_at`); unit reads (`Location`,
  `IsLive`) on the computer player's units are skipped.
- A beacon's variant is named as OP2Lua's `create_beacon` takes it: the game numbers a 1-bar beacon's High, Mid, Low
  0, 1, 2 and a 2- or 3-bar one's Mid, Low, High (Plymouth 12's Bar3 mines of variant 0 are Mid).
- `op2dll disasm --block DLL 0xADDR` prints a UnitBlock's units as the game holds them after the setup (sorted by
  its constructor, the order a mission's loops read them) as `ai.unit_block` records, and `--route` a
  PatrolRoute as `set_patrol` arguments, for code written by hand. `--block --shipped` prints the table as the DLL
  ships it, before the constructor sorts it in place: the input `ai.unit_block` needs to make the same block (the
  game sorts it again, and its sort doesn't keep the order of equal records, so a sorted table placed Eden 12's Hard
  waves differently).
- `op2dll lua` copies the mission's unit-mission flag (`DescBlock`'s `bUnitMission`) into the stub: the game then greys
  the base buttons and hides the stores, as in Tutorial 2. Colony games, tutorials and demos set only the colonists
  and stores the setup sets.
- `op2dll calls --repeat N`: call each `--call` export N times after a setup with the lobby's disasters on, one run
  after another, to see the random choices a trigger function makes.
- View > Disasters and timers: the mission's time triggers (when, whether they repeat, the function they call), and
  a forecast of the disasters its code sets off in its first 100 to 2000 marks: meteors, earthquakes, eruptions,
  storms, vortexes and blight, with when (a mark, or the window its random numbers choose in), where, size and the
  function that set it off. The forecast is op2-missionhost's replay of the mission with nobody playing, run in the
  background; disasters with a known place are marked on the map. `op2dll forecast` prints it.
- A mission type dropdown in the mission browser: campaign, colony, tutorial, auto demo, multiplayer, or one
  multiplayer mode.
- The mission browser lists a DLL once when the same file is also under OPU/base/maps (OPU copies every original
  mission there), at the game folder's path.
- The app icon (Tech_Planet.ico) on the exe and on every window.
- ARCHITECTURE.md and this changelog.

### Changed
- op2-rs pinned at `982336cf` (v1.0.2, was `2e1a7ffb`). Its mission host now has a stand-in `Hackaray.dll`
  (`SetNukePatch` and `SetMissionEndHook2` report success without patching anything), so a mission that loads it,
  such as a community map built on it, gets through its setup.
- op2-rs pinned at `2e1a7ffb` (was `b60d48b4`). Its mission host now runs community campaign mods that read
  files beside their DLL (read-only), ask op2ext for their folders, play their own music with BASS 2.2, find their
  own sections with imagehlp or call the game's message box.
- op2-rs pinned at `b60d48b4` (was `877421c5`). Its mission host now formats `scr_snprintf` (the game's own at
  `0x475930`), so missions' formatted texts (objectives such as "Store %d units of food", trigger names) read as the
  game shows them, where they came out empty or as old buffer contents. op2-formats is the crates.io crate 0.4, as op2-rs
  now uses it. Network starts are made with the Classic Rules and the original's limits, the new `NetStart` fields.
  Stock missions' setups and messages are unchanged (40 setups and 20 message runs compared, old against new).
- `op2dll lua` finds the mission's map in the game folder (`--game-dir`), and fails without it. Before, it never
  found it and took every map for 64 x 64, so on 512-wide maps (Axen's Home, Around the World...) every unit was
  written 32 tiles west of its place.
- op2-rs crates updated from 9f6b7d7 to 877421c (2026-11-04). Unit art tables are now read from the game folder's
  Outpost2.exe (GOG 1.2.0.7, or a CD copy updated by OPU 1.4.1); with another build, objects draw without art. Replays
  run differently in places (e11's reinforcements now arrive at mark 464, not 460), and multiplayer forecasts use
  the chosen player count. `mr4Yuko.dll` now gets past `LCMapStringEx` but runs out of steps in its setup.
- Trigger times are always given in marks: a time that isn't a whole mark shows its ticks as hundredths
  ("699.99 marks", not "69999 ticks"), and one mark reads "1 mark".

### Fixed
- The replay writes a flag (a unit's lights, day and night, a wreck's discovered flag) as `true`, `false` or
  `<expr> ~= 0`: a text replace afterwards could turn `set_daylight_everywhere(0 ~= 0)` into
  `set_daylight_everywherefalse` and change the end of a longer expression.
- `disasm --block` / `--route` write tiles with no padding on a 512-wide map, accept `0X` addresses, and
  `--shipped` without `--block` is an error.
- A converted computer player's units no longer count as "units made other ways than CreateUnit".
- A placement unit has no `garage` field (only `on_init`'s `make_unit` parks a vehicle).
- A trigger made once per player that calls an unconverted function keeps its note, as a single one does.
- A failed replay's reason comes under a "Not converted" header when there were no other notes.
- The usage text (`op2dll --help`) printed the `calls`, `disasm` and `lua` lines out of line, and gave
  `disasm --block` without its `--difficulty`.
- `op2dll lua` wrote a Lua string with a line break (the auto demos' victory text) across lines, which isn't valid Lua.
- `op2dll lua` ran a new layout's Medium and Low games on the seed that rolled it at High, but the resource level
  changes the mission's rolls, so those games often showed other layouts; copies of such a layout then had no units
  at Medium and Low (Peacekeeper's NW1, NW4 and SE2, twelve of La Corrida's). It now runs more seeds at each level
  until every layout shape turns up there.
- The usage text (`op2dll --help`) printed the `forecast` and `messages` lines out of line with the others.
- Guard posts armed with a Starflare or Supernova (`TURRET_DESTRUCT`, `TURRET_DESTRUCT_ADV`) drew as a plain square
  instead of their art.
- Agridomes, Common and Rare Storage, Garages and mines were missing the dock pad (or base) the game draws under
  them as a second picture.
- Structures drew up to half a tile out of place: the art was anchored the same way for every size, not at the
  footprint's centre as the game does (2x2 buildings such as the Forum sat half a tile right, 3x3 ones half a tile
  high). A selected structure is now outlined over its whole footprint.

## [0.1.0] - 2026-10-30

First version.

### Added
- Opens an Outpost 2 mission DLL and checks that it is one: a 32-bit x86 DLL with `InitProc` and a mission
  descriptor. Shows the name, type, players, tech level, map and tech tree.
- Shows the mission's map with its tilesets: pan, zoom, tile grid, and an overview image when zoomed far out.
- Runs the mission's setup (`InitProc`) in op2-rs's x86 sandbox and draws the objects it creates with the game's art:
  units, structures, walls and tubes, magma vents, fumaroles, and mining beacons. Common and rare beacons always
  show, with their yield bars.
- Objects panel: a details card for the selected object (weapon or truck cargo, facing, lights, ore type, bars...),
  a list per player with a checkbox to hide that player, layer toggles, and a hover tooltip.
- Mission browser in its own window. It lists every mission DLL under the game folder, including OPU's `maps`,
  read in the background. It has a filter, Up/Down steps through the list, and M shows or hides the window.
- Save the view (Ctrl+S) or the whole map (Ctrl+Shift+S) as PNG.
- Command line:
  - `inspect` prints a DLL or folder as text or JSON; `--objects` includes the setup.
  - `render` saves the whole map with its objects.
  - `--difficulty`, `--players`, `--game-dir`, `--center` and `--screenshot` options.
- The game folder comes from `--game-dir`, `$OP2_DIR`, op2-rs's settings or the usual install places. It can also be
  set from the File menu or by dropping the folder onto the window.
- `OP2_BUDGET` raises the setup's instruction limit, as in op2-rs's tools.
- Windows release builds open no console window. They use the console they were started from, and show errors in a
  message box otherwise.

### Known issues
- `mr4Yuko.dll` (OPU YukonTrail): the setup stops at `kernel32!LCMapStringEx`, which op2-missionhost only stubs.
