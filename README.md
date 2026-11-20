# OP2MissionReader

OP2MissionReader (the program is `op2dll`) opens an Outpost 2 mission DLL, checks that it is one, and shows its
details, its map and the objects its setup creates: units, structures, walls and tubes, mining beacons, magma vents
and fumaroles, drawn with the game's own art.

It doesn't contain any of the game's files: it reads the map, tilesets and art from your Outpost 2 install (GOG 1.2
with OPU 1.4.1 is what op2-rs is tested against). The mission's setup (`InitProc`) runs in op2-rs's x86 sandbox, not
natively, so this works on any platform op2-rs builds on.

**op2-missionhost is what makes this project possible.** It is op2-rs's mission host: it loads a mission DLL into
op2-rs's x86 emulator (op2-x86) and answers the DLL's calls into the game (creating units, triggers, AI groups and the
rest) the way Outpost 2 does, backed by op2-rs's simulation (op2-sim). Everything OP2MissionReader shows of a
mission's setup, and everything it converts to Lua, comes from running the mission there.

## Using it

```
op2dll [--game-dir DIR] [--center X,Y] [--screenshot OUT.png] [DLL]
op2dll inspect [--json] [--objects] [--difficulty 0-2] [--players N] [--game-dir DIR] DLL|FOLDER
op2dll render [--difficulty 0-2] [--players N] [--size PX] [--jpg] [--game-dir DIR] DLL OUT.png|.jpg | FOLDER OUTDIR
op2dll opm [--difficulty 0-2] [--players N] [--game-dir DIR] DLL OUT.opm
op2dll forecast [--json] [--marks N] [--difficulty 0-2] [--game-dir DIR] DLL
op2dll messages [--json] [--marks N] [--difficulty 0-2] [--players N] [--game-dir DIR] DLL
op2dll calls [--start start_info.json] [--marks N] [--call EXPORT] [--repeat N] [--difficulty 0-2] [--players N] [--game-dir DIR] DLL
op2dll lua [--simple] [--stub LuaMission.dll] [--game-dir DIR] DLL OUTDIR
```

The mission browser opens in a window of its own beside the main one. It lists every mission DLL in the game folder
and the folders under it (OPU's `maps` included), with a text filter and a mission type dropdown (campaign, colony,
tutorial, auto demo, multiplayer or one multiplayer mode); click one to open it.

The game folder is `--game-dir`, else `$OP2_DIR`, else the one op2-rs saved, else the usual install places; it can also
be set in the window (File > Set game folder, or drop the folder onto the window). A DLL given by name alone is looked
up in the game folder.

| Key | |
|---|---|
| Up / Down | previous / next mission in the list |
| M | the mission browser window |
| O, A | objects; art or dots |
| V, S, R, W | vehicles, structures, beacons and vents, walls and tubes |
| G, F | tile grid; fit the map |
| Ctrl+O | open a DLL |
| Ctrl+S, Ctrl+Shift+S | save the view as PNG, or the whole map as PNG or JPEG (at full size or the size chosen under "Whole map size": 4096, 2048, 1024 or 512 px) |
| Ctrl+E | export the mission as an .opm file |

Drag to pan, wheel to zoom. Hover an object for its details; click it (on the map or in the list) to select it. The
checkbox by each player hides that player's objects.

The Players table shows what each player starts with: workers, scientists, kids, ore, food, solar satellites, morale
and research done. An object's details show what it holds: a Cargo Truck's load, a Structure Factory's or
Spaceport's bays, a Garage's vehicles. Vehicles parked in a Garage are listed (marked `[in a garage]`) but not drawn.

Five more windows open from the View menu:
- **Objectives and triggers**: the mission's victory and failure conditions with what each waits on, then every
  trigger the setup made: what it watches, whether it repeats, and the mission function it calls. Click a trigger
  with an area (a rectangle, point or escape zone) to outline it on the map and centre there; View > Trigger areas
  on the map outlines them all (off by default). Triggers the mission makes later, from its own code, aren't listed.
- **AI groups**: the computer players' fight, mining and building groups the setup made: what each is told, its
  units, the units it wants, and a building group's plans (the structures, mines, walls and tubes it rebuilds). Click
  a group to outline its area, patrol route and units on the map and draw its plans as dashed outlines (green where
  nothing stands yet). Groups the mission makes later, from a trigger or its AI code, aren't listed.
- **Disasters and timers**: the mission's time triggers, then a forecast of the disasters its code sets off
  (meteors, earthquakes, eruptions, storms, vortexes, blight) in its first 100 to 2000 marks: when, where, how big,
  and the mission function that set it off. Click one to centre the map on it; disasters with a known place are
  marked on the map. The forecast is op2-rs's replay of the mission with nobody playing, so it's a prediction:
  triggers that wait on the player don't fire, a mission may fail early with nobody playing (the window says when),
  and random times and places are given as windows or "random".
- **Messages**: every message text the mission can post, read from the DLL whether or not anything triggers it, with
  the exported function that posts it and what calls that function: the setup, `AIProc`, or a trigger and what it
  waits on (a time, a vehicle count, a scout beside a lab...). Then the messages op2-rs's replay with nobody playing
  posts: when, to whom, and the mission function that posted it.
  Most campaign messages answer the player (a building finished, a unit lost), so the replay shows only some; click
  one with a place to centre the map there. The replay length is shared with the disasters window.
- **Research**: each player's research done at the start, by name from the mission's tech tree (the file next to
  the DLL if there is one, else the game's).

`inspect` prints what the window shows as text or JSON; on a folder it prints one line per DLL. With `--objects` it also
runs each mission's setup and lists what it creates, with a line counting its resources (ore beacons by ore and
yield, magma vents, fumaroles, wrecks; `resources` in the JSON). `forecast` prints the disaster forecast and `messages` the
mission's messages (600 marks of replay unless `--marks`). `render` saves the whole map with everything on it as a PNG
(32 px per tile, less for maps wider than 256 tiles): the map as the mission's setup leaves it, as the game starts it
(normal difficulty unless `--difficulty`; a multiplayer map with every player slot it has unless `--players`). On a
folder it saves one `NAME.png` per mission DLL into OUTDIR, skips DLLs that aren't missions, reports a mission whose
setup fails and goes on, e.g. `op2dll render --game-dir "C:\Games\Outpost 2" "C:\Games\Outpost 2" shots` for
every campaign, colony and multiplayer map's start. Full size is large (a 256x256 map is 8192 px, about 45 MB as a
PNG): `--size PX` shrinks the finished image so its longer side is at most PX, and an output name ending `.jpg` (or
`--jpg` on a folder) saves a JPEG, e.g. `--size 1024 --jpg` for pictures for the web.

`calls` prints every game API call the mission makes, in order, with its arguments and result: its setup, and with
`--marks` the replay after it. `--start` starts a multiplayer mission as a network game's lobby agreed it, from
op2-rs's `start_info.json` (seed, each player's colony, colour and resource level, day and night, disasters, Morale
Steady, initial vehicles), so runs with different lobby choices can be compared to learn what a mission does with each.
Tug-O-War in the OP2LuaSDK was remade this way from `ml2_31.dll`. Coordinates in it are the game's internal ones
(x + 32 on maps narrower than 512 tiles). `--call EXPORT` (repeatable) then calls one of the mission's exported
functions, as if a trigger had fired it, to see what it does; with `--repeat N` each is called N times in a row (after
a setup with disasters on), to see the random choices it makes.

`lua` converts a multiplayer mission into an OP2Lua mission in `OUTDIR`: `<Name>Lua.placement.lua`, `<Name>Lua.lua` and
a `<Name>Lua.dll` made from OP2Lua's stub (`--stub`, else `$OP2LUA_STUB`, else `LuaMission.dll` beside op2dll). It is
written from what the mission does, not from its code: the setup runs hundreds of times, for every player count, lobby
choice and enough seeds to see every base layout, and the runs are compared. The Lua has the base layouts and who gets
them, units by resource level and colony (with Cargo Trucks' loads), the ways a Land Rush column turns, initial vehicles, beacons, tubes, day and night, morale, and the win (Last
One Standing, Space Race, Resource Race, Midas, Land Rush), and the disasters and other timed events: each function a
trigger calls is run 600 times and remade with the same draws of the game's random numbers (disasters, a tile rolled
again until it's outside a rectangle, the next time trigger). A computer player (Peacekeeper's, La Corrida's) is
set up as the mission sets it up, with the game's AI groups (OP2Lua's `ai.*`), and the functions that add to its
base later are written from repeated calls. What it couldn't convert (trigger functions that do something else,
lava, objects nobody owns) is listed at the end of the Lua and printed. It needs the game folder, for
the map.

`lua --simple` writes a simpler version instead (`<Name>Simple`, "(Lua, simple)" in the menu): each area has one base,
the same every game, written as flat tables of structures, vehicles, tubes and beacons. The base is the area's first
layout whose structures all stand on passable ground (some originals put one on rock and the game moves it off),
and a Land Rush column is laid out the first way whose vehicles all stand on passable ground.
Every resource level gets High's units, and beacons the original rolls are one fixed pick; who gets which area, the
win, disasters, day and night, morale and the computer player are as in the full conversion.

Game time is given in marks, as the game's clock shows it (a mark is 100 ticks). A mission that needs a helper DLL
beside it (OPU's DangerZone imports `op2extra.dll`) still opens without it, with a warning: the helper's functions are
stubs, so what they would have set up may be missing.

### Exporting to .opm

File > Export as .opm (Ctrl+E), or `op2dll opm`, saves the mission's starting state as an `.opm` file, the JSON
format of OP2MissionEditor and the OP2DotNetMissionSDK. It holds what the setup made in the run shown (the difficulty
and player count chosen): the mission's details, each player's colony, colour, resources, research and camera, their
units and structures, walls and tubes, beacons, markers, wreckage, daylight and music.

Some of a mission can't be expressed in the format, or isn't known after its setup has run. That is left out, and the
export says what:
- the mission's own code: its AI, triggers, victory conditions and disasters. Computer players get `BotType` None;
- what factories and spaceports hold in their bays;
- units owned by no player.

Walls and tubes belong to nobody in the game, but the format only has them per player, so they go with player 0. A
mine is written with its beacon's yield and variant, and that beacon isn't written separately, because the SDK makes
a mine's beacon itself. Positions are the SDK's: the map file's tile plus one on both axes.

## Building

Needs Rust 1.99 (see `rust-toolchain.toml`).

```
cargo build --release
```

The op2-rs crates come from gitlab.com/bufke/op2-rs at a fixed commit (Cargo.toml); op2-formats comes from crates.io
(0.6; op2-rs's crates bring in their own 0.4). To build against a local clone instead, add to `.cargo/config.toml`
(not committed):

```toml
[patch."https://gitlab.com/bufke/op2-rs"]
op2-x86 = { path = "../op2-rs/tools/crates/op2-x86" }
op2-missionhost = { path = "../op2-rs/tools/crates/op2-missionhost" }
op2-settings = { path = "../op2-rs/tools/crates/op2-settings" }
op2-sim = { path = "../op2-rs/tools/crates/op2-sim" }
```

The drawing and setup code is also a library (`src/lib.rs`, crate `op2dll`) that other programs use without the
window: [OP2Imager](https://github.com/leviathan400/OP2Imager) (`../op2imager`) depends on it with `default-features = false`, which leaves out eframe and the
other windowing libraries; the `op2dll` program needs the `app` feature, on by default.

## References

- [op2-rs](https://gitlab.com/bufke/op2-rs) (GPL-3.0-or-later): the Outpost 2 reimplementation whose crates this
  builds on.
  - **op2-missionhost**: runs a mission DLL in the emulator and answers its game API calls; the core of this project.
  - **op2-x86**: the PE loader and x86 emulator the DLL runs in.
  - **op2-sim**: the game simulation behind the API (units, structures, terrain, the computer players' groups).
  - **op2-settings**: the game folder op2-rs saved.
- [op2-formats](https://crates.io/crates/op2-formats): reads the game's files (maps, tilesets, unit art, sheets,
  `.vol` archives, `Outpost2.exe`'s tables).
- [OP2Imager](https://github.com/leviathan400/OP2Imager): batch images of maps, mission DLLs, saved games and
  OP2TestHarness states, using this project's library.
- [OP2TestHarness](https://github.com/leviathan400/OP2TestHarness): runs missions in the real game and records what
  they do; its states and whole-map images are what the exact drawing (`exact.rs`) is checked against.
- [OP2Lua](https://github.com/leviathan400/OP2Lua): the Lua mission runtime that `op2dll lua` writes missions for.
- [Outpost Universe](https://www.outpost2.net/): the Outpost 2 community and OPU, the game update this is tested with.

## Licence

GPL-3.0-or-later, as op2-rs.
