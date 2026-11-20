//! `op2dll calls`: every game API call a mission makes, in order, with its arguments and result: its setup
//! (`InitProc`) and, with `--marks`, the replay after it. A multiplayer mission can be started as a network game's
//! lobby agreed it (`--start`, op2-rs's `start_info.json`: seed, races, colours, resource levels, day and night,
//! disasters, morale, initial vehicles), so runs with different lobby choices can be compared call by call. The
//! helpers here also call trigger functions after the setup, for the Lua conversion, and read the mission's memory.

use std::path::Path;

use anyhow::{Context, Result};
use op2_missionhost::play::Play;
use op2_missionhost::runtime::{CallRec, NetStart, Options};

use crate::disasters::MARK;
use crate::setup::StartOptions;

/// The calls of the setup and the first `marks` marks, started from `start`, or from the lobby in `lobby` (a
/// `start_info.json`), which then decides the player count and the seed too. Each of `exports` (trigger functions,
/// say) is then called once, in order, as if its trigger had fired.
pub fn record(dll: &Path, start: StartOptions, lobby: Option<&Path>, marks: u32, exports: &[String]) -> Result<Vec<CallRec>> {
    let base = Options { seed: 0, difficulty: start.difficulty, players: start.players, ..Options::from_env() };
    let opts = match lobby {
        Some(path) => {
            let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            let json: serde_json::Value = serde_json::from_str(&text).with_context(|| format!("{} isn't JSON", path.display()))?;
            NetStart::from_json(&json)?.options(base)
        }
        None => base,
    };
    record_with(dll, &opts, marks, exports)
}

/// [`record`] from game options as they are.
pub fn record_with(dll: &Path, opts: &Options, marks: u32, exports: &[String]) -> Result<Vec<CallRec>> {
    let mut play = Play::new(dll, "", opts)?;
    while play.tick() < marks * MARK && play.world().end.is_none() {
        play.step()?;
    }
    for name in exports {
        match play.ms.call_export(name) {
            Some(r) => { r.map_err(|e| anyhow::anyhow!("{name}: {e:?}"))?; }
            None => anyhow::bail!("the mission exports no {name}"),
        }
    }
    Ok(std::mem::take(&mut play.ms.rt.log))
}

/// A network game of `players` from `seed` with the lobby's disasters on, the rest as the lobby's defaults, logging
/// the calls.
pub fn disasters_on(players: u8, seed: u32) -> Options {
    let start = op2_missionhost::runtime::NetStart {
        seed,
        slots: (0..players as i32).map(|i| op2_missionhost::runtime::NetSlot { eden: true, color: i, resources: 0 }).collect(),
        day_night: true,
        disasters: true,
        morale_steady: true,
        initial_vehicles: 0,
        lift_limits: false,
        classic_rules: true,
    };
    start.options(Options { call_log: true, ..Options::from_env() })
}

/// `count` dwords of the mission's memory from `addr`, after its setup: the data its code reads (rectangles, tables),
/// which the DLL's static constructors may fill in as it loads.
pub fn peek(dll: &Path, opts: &Options, addr: u32, count: usize) -> Result<Vec<u32>> {
    let play = Play::new(dll, "", opts)?;
    Ok((0..count as u32).map(|i| play.ms.m.read_u32(addr + 4 * i)).collect())
}

/// The calls of `times` runs of the export `name` after the setup, one list per run: a trigger function's random
/// choices show up across runs (the game's random numbers go on from one to the next).
pub fn repeat(dll: &Path, opts: &Options, name: &str, times: usize) -> Result<Vec<Vec<CallRec>>> {
    let mut play = Play::new(dll, "", opts)?;
    // Drop the setup's calls: each run's list holds that run's only.
    std::mem::take(&mut play.ms.rt.log);
    let mut runs = Vec::with_capacity(times);
    for _ in 0..times {
        match play.ms.call_export(name) {
            Some(r) => { r.map_err(|e| anyhow::anyhow!("{name}: {e:?}"))?; }
            None => anyhow::bail!("the mission exports no {name}"),
        }
        runs.push(std::mem::take(&mut play.ms.rt.log));
    }
    Ok(runs)
}

/// `calls` calls of `name` in each of `games` games, for a function that goes quiet for good once it has acted:
/// game `i` draws `i` numbers before its first call, so each game sees its own stretch of the random sequence (the
/// setup's own draws depend on the seed only, and leave some values out).
pub fn games_of(dll: &Path, opts: &Options, name: &str, games: usize, calls: usize) -> Result<Vec<Vec<Vec<CallRec>>>> {
    let mut out = Vec::with_capacity(games);
    for i in 0..games {
        let mut play = Play::new(dll, "", opts)?;
        // Draw `i` numbers from the game's random sequence, discarding them.
        for _ in 0..i {
            play.ms.rt.world.rand(1 << 15);
        }
        std::mem::take(&mut play.ms.rt.log);
        let mut runs = Vec::with_capacity(calls);
        for _ in 0..calls {
            match play.ms.call_export(name) {
                Some(r) => {
                    r.map_err(|e| anyhow::anyhow!("{name}: {e:?}"))?;
                }
                None => anyhow::bail!("the mission exports no {name}"),
            }
            runs.push(std::mem::take(&mut play.ms.rt.log));
        }
        out.push(runs);
    }
    Ok(out)
}

/// Two calls of `name` in one game whose players' population is set to `people` first (kids, workers and scientists,
/// by player; 0 for players not listed), for a function that waits on the colonies' size.
pub fn with_population(dll: &Path, opts: &Options, name: &str, people: &[[i32; 3]]) -> Result<[Vec<CallRec>; 2]> {
    let mut play = Play::new(dll, "", opts)?;
    for (i, p) in play.ms.rt.world.players.iter_mut().enumerate() {
        let [kids, workers, scientists] = people.get(i).copied().unwrap_or([0; 3]);
        (p.kids, p.workers, p.scientists) = (kids, workers, scientists);
    }
    std::mem::take(&mut play.ms.rt.log);
    let mut call = || -> Result<Vec<CallRec>> {
        match play.ms.call_export(name) {
            Some(r) => {
                r.map_err(|e| anyhow::anyhow!("{name}: {e:?}"))?;
            }
            None => anyhow::bail!("the mission exports no {name}"),
        }
        Ok(std::mem::take(&mut play.ms.rt.log))
    };
    let first = call()?;
    Ok([first, call()?])
}

/// One call as a line: "tick phase Class::Function(args) -> result".
pub fn line(c: &CallRec) -> String {
    let ret = if c.ret.is_empty() { String::new() } else { format!(" -> {}", c.ret) };
    let stub = if c.stub { "  [stub]" } else { "" };
    format!("{:>6} {:<8} {}({}){ret}{stub}", c.tick, c.phase, c.api, c.args.join(", "))
}
