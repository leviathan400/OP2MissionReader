//! Disasters and other timed events of a converted mission (`op2dll lua`): the functions the mission's time triggers
//! (and others) call, remade in Lua from what they do, not from their code.
//!
//! Each function is called many times after a setup (`calls::repeat`): a multiplayer mission's in a lobby with
//! disasters on, a single-player mission's at a difficulty (at each of the three when it asks for it), and its calls
//! compared run by run. The game's random numbers (`GetRand(n)`) are the inputs; every other call is an output:
//!
//! - a disaster (`SetMeteor`, `SetEarthquake`, `SetEruption`, `SetLightning`, `SetTornado`) whose arguments are each
//!   a fixed number, a draw times a number plus a number, the sum of two draws, or a fixed table indexed by a draw;
//!   likewise a tile change (`GameMap::SetTile`, the volcano tiles set before an eruption; `InitialSetTile`,
//!   `SetCellType`), a tile marked where lava may flow (`GameMap::SetLavaPossible`), the Blight put on or taken off a
//!   tile (`GameMap::SetVirusUL`) and its speed (`SetMicrobeSpreadSpeed`);
//! - a message (`AddMessage`);
//! - a rectangle test (`MAP_RECT::Check`) after a few draws, redone while it says the tile is inside: a tile rolled
//!   again until it's outside a base, say;
//! - a new time or count trigger (`CreateTimeTrigger`, `CreateCountTrigger`) and the end of the one that fired
//!   (`ScStub::Destroy`), so the next disaster comes after a new random wait.
//!
//! When runs differ in what they call (a meteor in one, a storm in another), a draw before the difference that
//! decides it is found and becomes an `if`. A function that only reads the colonies' size is a population rule,
//! measured at chosen populations instead. In a multiplayer mission, reads of the game after a function's last
//! action that led to nothing are left out, and the Lua says so. A function with any other call, or whose arguments
//! follow none of these rules, isn't converted: its triggers are still made, calling an empty function, and the
//! caller lists why.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use op2_missionhost::api::TriggerSpec;
use op2_missionhost::runtime::CallRec;

use crate::triggers::Trigger;

/// Runs of each function: enough that a draw of up to 32 shows each value in every branch.
const RUNS: usize = 600;
/// A table indexed by a draw is used for draws of at most this many values.
const TABLE_MAX: i64 = 32;

/// The Lua of the converted functions and of the triggers that call them.
pub struct Timed {
    /// Top-level Lua, before `on_init`: the functions.
    pub functions: String,
    /// Lines for `on_init`: the triggers.
    pub init: String,
    /// The triggers made whatever the lobby, each with the number the setup's trigger has, in the setup's order.
    pub init_by_id: Vec<(u32, String)>,
    /// The setup's triggers covered (not those that call an empty function).
    pub handled: BTreeSet<u32>,
    /// The functions converted, for [`lines_for`].
    pub converted: BTreeSet<String>,
    /// The functions made empty because they aren't converted, for [`lines_for`].
    pub stubs: BTreeSet<String>,
}

/// The disaster, tile, lava and Blight calls converted, by the game's names.
const DISASTERS: [&str; 11] = [
    "TethysGame::SetMeteor",
    "TethysGame::SetEarthquake",
    "TethysGame::SetEruption",
    "TethysGame::SetLightning",
    "TethysGame::SetTornado",
    "GameMap::SetTile",
    "GameMap::SetLavaPossible",
    "GameMap::InitialSetTile",
    "GameMap::SetCellType",
    "GameMap::SetVirusUL",
    "TethysGame::SetMicrobeSpreadSpeed",
];

/// One step of a run as the Lua writer sees it: a draw, a rectangle loop, or something it does.
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// `GetRand(n)`.
    Rand(i64),
    /// Draws of these sizes, then a rectangle test of the tile they make, redone while it says `again`.
    Loop { rect: [i64; 4], sizes: Vec<i64> },
    /// A call of [`DISASTERS`].
    Effect(&'static str),
    /// `CreateTimeTrigger` with its numbers and function.
    Timer(Vec<i64>, String),
    /// Another trigger made (`CreateCountTrigger`): its Lua and the function it calls.
    Made(String, String),
    /// `ScStub::Destroy` of the trigger that fired.
    Destroy,
    /// A line of Lua that draws nothing and makes nothing (a message).
    Line(String),
}

/// One try of a rectangle test: its draws, the tile tested and the answer.
type Try = (Vec<i64>, (i64, i64), bool);

/// One run, read: its tokens and, per token, the numbers it saw.
#[derive(Debug, Clone)]
struct Run {
    /// The steps, in the order the run made them.
    toks: Vec<Tok>,
    /// The value of each draw, in order (a loop's draws: the last try's).
    vals: Vec<i64>,
    /// Per `Effect`/`Timer` token, its numeric arguments.
    args: BTreeMap<usize, Vec<i64>>,
    /// Per `Loop` token, every try: its draws, the tile tested and the answer.
    tries: BTreeMap<usize, Vec<Try>>,
}

/// The integers in a call's argument text, in order.
fn ints(s: &str) -> Vec<i64> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_digit() || (ch == '-' && cur.is_empty()) {
            cur.push(ch);
        } else {
            if let Ok(v) = cur.parse() {
                out.push(v);
            }
            cur.clear();
        }
    }
    out
}

/// A raw item of a run, before rectangle tests are grouped into loops.
enum Item {
    /// `GetRand(n)` and the number drawn.
    Rand(i64, i64),
    /// `MAP_RECT::Check`: the rectangle, the tile tested and whether it's inside.
    Check([i64; 4], (i64, i64), bool),
    /// A call of [`DISASTERS`] and its numbers.
    Effect(&'static str, Vec<i64>),
    /// `CreateTimeTrigger`: its numbers, its function and the trigger it made.
    Timer(Vec<i64>, String, i64),
    /// `CreateCountTrigger`: its Lua and its function.
    Made(String, String),
    /// `ScStub::Destroy` and the trigger it ends.
    Destroy(i64),
    /// A message's Lua.
    Line(String),
}

/// The Lua of a `TethysGame::AddMessage` call: "pixel_x, pixel_y, \"text\", recipient, sound" (recipient -1 for
/// everyone, written as `nil`; a pixel of -1 points nowhere), as `game.message_at`.
fn message(all: &str) -> Result<String, String> {
    let bad = || format!("AddMessage({all})");
    let (head, rest) = all.split_once('"').ok_or_else(bad)?;
    let (text, tail) = rest.rsplit_once('"').ok_or_else(bad)?;
    let num = |s: &str| s.trim().parse::<i64>().map_err(|_| bad());
    let px: Vec<&str> = head.split(',').map(str::trim).filter(|v| !v.is_empty()).collect();
    let rt: Vec<&str> = tail.split(',').map(str::trim).filter(|v| !v.is_empty()).collect();
    let ([x, y], [to, sound]) = (&px[..], &rt[..]) else { return Err(bad()) };
    let to = num(to)?;
    let player = if to < 0 { "nil".to_string() } else { (to + 1).to_string() };
    Ok(format!("game.message_at({}, {}, {player}, {}, {})", crate::luaexport::quoted(text), num(sound)?, num(x)?, num(y)?))
}

/// The Lua of a `CreateCountTrigger` call made in a function (River of Fire's `OneMine` waits on the second player's
/// first mine): "enabled, one_shot, player, TYPE, CARGO, count, compare, \"Function\"".
fn count_trigger(all: &str) -> Result<(String, String), String> {
    let name = all.split('"').nth(1).ok_or("CreateCountTrigger without a function")?.to_string();
    let f: Vec<&str> = all.split('"').next().unwrap_or("").split(',').map(str::trim).collect();
    let [enabled, one_shot, player, ty, cargo, count, cmp, ..] = f[..] else {
        return Err(format!("CreateCountTrigger({all})"));
    };
    let n = |s: &str| s.parse::<i32>().map_err(|_| format!("CreateCountTrigger({all})"));
    // "None" is mapNone (0), which the stock maps pass, as the setup's count triggers write it.
    if cargo != "None" {
        return Err("a count trigger on a cargo".into());
    }
    let player = n(player)?;
    let t = Trigger {
        id: 0,
        kind: "CountTrigger".into(),
        what: String::new(),
        enabled: n(enabled)? != 0,
        one_shot: n(one_shot)? != 0,
        callback: Some(name.clone()),
        objective: None,
        area: None,
        spec: TriggerSpec::Count {
            player,
            ty: op2_missionhost::api::MapId(crate::luaexport::map_id(ty)),
            cargo: 0,
            count: n(count)?,
            cmp: op2_missionhost::api::Compare::from_legacy(n(cmp)?),
        },
    };
    let who = (player >= 0).then(|| (player + 1).to_string());
    let lua = crate::victory::constructor(&t, who.as_deref(), &BTreeMap::new(), &format!("on = {name}"))?;
    Ok((lua, name))
}

/// A run's calls as items, or the call that keeps the function from being converted.
fn items(run: &[CallRec]) -> Result<Vec<Item>, String> {
    let mut out = Vec::new();
    for c in run {
        let all = c.args.join(", ");
        match c.api.as_str() {
            "ScStub::~ScStub" => {}
            "TethysGame::GetRand" => {
                let n = *ints(&all).first().ok_or("GetRand without a number")?;
                out.push(Item::Rand(n, c.ret.trim().parse().map_err(|_| "GetRand without a result")?));
            }
            "MAP_RECT::Check" => {
                let v = ints(&all);
                if v.len() != 6 {
                    return Err(format!("MAP_RECT::Check({all})"));
                }
                out.push(Item::Check([v[0], v[1], v[2], v[3]], (v[4], v[5]), c.ret.trim() != "0"));
            }
            "CreateTimeTrigger" => {
                let name = all.split('"').nth(1).ok_or("CreateTimeTrigger without a function")?.to_string();
                let id = ints(&c.ret).first().copied().unwrap_or(-1);
                out.push(Item::Timer(ints(all.split('"').next().unwrap_or("")), name, id));
            }
            "CreateCountTrigger" => {
                let (lua, name) = count_trigger(&all)?;
                out.push(Item::Made(lua, name));
            }
            "ScStub::Destroy" => out.push(Item::Destroy(ints(&all).first().copied().unwrap_or(-1))),
            "TethysGame::AddMessage" => out.push(Item::Line(message(&all)?)),
            api => match DISASTERS.iter().find(|d| **d == api) {
                Some(d) => out.push(Item::Effect(d, ints(&all))),
                None => return Err(format!("it calls {}", api.trim_start_matches("TethysGame::"))),
            },
        }
    }
    Ok(out)
}

/// How many draws each rectangle's tries make: the fewest seen between two tests of it in a row (a loop's first try
/// can come after other draws, which aren't its own).
fn try_sizes(items: &[Vec<Item>]) -> BTreeMap<[i64; 4], usize> {
    let mut sizes: BTreeMap<[i64; 4], usize> = BTreeMap::new();
    for run in items {
        let mut last: Option<[i64; 4]> = None;
        let mut draws = 0;
        for it in run {
            match it {
                Item::Rand(..) => draws += 1,
                Item::Check(rect, ..) => {
                    if last == Some(*rect) && draws > 0 {
                        let k = sizes.entry(*rect).or_insert(draws);
                        *k = (*k).min(draws);
                    }
                    last = Some(*rect);
                    draws = 0;
                }
                _ => {
                    last = None;
                    draws = 0;
                }
            }
        }
    }
    sizes
}

/// A run's items as tokens: each rectangle test, with its draws just before it (`size` of them; all, for a rectangle
/// never tested twice in a row), is one try; tries of the same draws and rectangle follow each other while the
/// answer is `again`.
fn tokens(items: &[Item], again: bool, size: &BTreeMap<[i64; 4], usize>) -> Run {
    let mut run = Run { toks: Vec::new(), vals: Vec::new(), args: BTreeMap::new(), tries: BTreeMap::new() };
    let mut pending: Vec<(i64, i64)> = Vec::new();
    let mut open: Option<usize> = None;
    let flush = |run: &mut Run, pending: &mut Vec<(i64, i64)>| {
        for (n, v) in pending.drain(..) {
            run.toks.push(Tok::Rand(n));
            run.vals.push(v);
        }
    };
    for it in items {
        match it {
            Item::Rand(n, v) => pending.push((*n, *v)),
            Item::Check(rect, at, inside) => {
                // Draws before the try's own are plain draws.
                let own = size.get(rect).copied().unwrap_or(pending.len()).min(pending.len());
                let mut before: Vec<(i64, i64)> = pending.drain(..pending.len() - own).collect();
                if !before.is_empty() {
                    open = None;
                    flush(&mut run, &mut before);
                }
                let sizes: Vec<i64> = pending.iter().map(|p| p.0).collect();
                let draws: Vec<i64> = pending.iter().map(|p| p.1).collect();
                pending.clear();
                let tok = Tok::Loop { rect: *rect, sizes };
                match open {
                    Some(i) if run.toks[i] == tok => {}
                    _ => {
                        run.toks.push(tok);
                        open = Some(run.toks.len() - 1);
                        run.vals.extend(std::iter::repeat_n(0, draws.len()));
                    }
                }
                let i = open.unwrap_or_default();
                let start = run.vals.len() - draws.len();
                run.vals[start..].copy_from_slice(&draws);
                run.tries.entry(i).or_default().push((draws, *at, *inside));
                if *inside != again {
                    open = None;
                }
            }
            Item::Effect(api, args) => {
                flush(&mut run, &mut pending);
                open = None;
                run.args.insert(run.toks.len(), args.clone());
                run.toks.push(Tok::Effect(api));
            }
            Item::Timer(args, name, _) => {
                flush(&mut run, &mut pending);
                open = None;
                run.args.insert(run.toks.len(), args.clone());
                run.toks.push(Tok::Timer(args.clone(), name.clone()));
            }
            Item::Made(lua, name) => {
                flush(&mut run, &mut pending);
                open = None;
                run.toks.push(Tok::Made(lua.clone(), name.clone()));
            }
            Item::Line(lua) => {
                flush(&mut run, &mut pending);
                open = None;
                run.toks.push(Tok::Line(lua.clone()));
            }
            Item::Destroy(_) => {
                flush(&mut run, &mut pending);
                open = None;
                run.toks.push(Tok::Destroy);
            }
        }
    }
    flush(&mut run, &mut pending);
    run
}

/// A number as a function of the draws made so far.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Expr {
    /// The same number in every run.
    Const(i64),
    /// `a * r[j] + b`.
    Lin(i64, usize, i64),
    /// `a * r[j] + c * r[k] + b`.
    Lin2(i64, usize, i64, usize, i64),
    /// `table[r[j]]`.
    Table(usize, Vec<i64>),
}

impl Expr {
    /// The number plus `d`.
    fn plus(self, d: i64) -> Expr {
        match self {
            Expr::Const(b) => Expr::Const(b + d),
            Expr::Lin(a, j, b) => Expr::Lin(a, j, b + d),
            Expr::Lin2(a, j, c, k, b) => Expr::Lin2(a, j, c, k, b + d),
            Expr::Table(j, t) => Expr::Table(j, t.into_iter().map(|v| v + d).collect()),
        }
    }

    /// The number for the draws `r` (0 for a value past a table's end).
    fn eval(&self, r: &[i64]) -> i64 {
        match self {
            Expr::Const(b) => *b,
            Expr::Lin(a, j, b) => a * r[*j] + b,
            Expr::Lin2(a, j, c, k, b) => a * r[*j] + c * r[*k] + b,
            Expr::Table(j, t) => t.get(r[*j] as usize).copied().unwrap_or(0),
        }
    }

    /// The number as a Lua expression, the draws named by `vars` (a table is indexed from 1).
    pub(crate) fn lua(&self, vars: &[String]) -> String {
        let term = |a: i64, v: &str| match a {
            1 => v.to_string(),
            -1 => format!("-{v}"),
            a => format!("{a} * {v}"),
        };
        let plus = |s: String, b: i64| match b {
            0 => s,
            b if b < 0 => format!("{s} - {}", -b),
            b => format!("{s} + {b}"),
        };
        match self {
            Expr::Const(b) => b.to_string(),
            Expr::Lin(a, j, b) => plus(term(*a, &vars[*j]), *b),
            Expr::Lin2(a, j, c, k, b) => {
                let second = term(*c, &vars[*k]);
                let joined = match second.strip_prefix('-') {
                    Some(s) => format!("{} - {s}", term(*a, &vars[*j])),
                    None => format!("{} + {second}", term(*a, &vars[*j])),
                };
                plus(joined, *b)
            }
            Expr::Table(j, t) => format!("({{ {} }})[{} + 1]", t.iter().map(i64::to_string).collect::<Vec<_>>().join(", "), vars[*j]),
        }
    }
}

/// The rule `samples` (the draws so far, the number) follow, if one of [`Expr`]'s does. `sizes` are the draws' sizes.
pub(crate) fn fit(samples: &[(&[i64], i64)], sizes: &[i64]) -> Option<Expr> {
    let first = samples.first()?.1;
    if samples.iter().all(|s| s.1 == first) {
        return Some(Expr::Const(first));
    }
    let nv = sizes.len();
    let check = |e: &Expr| samples.iter().all(|(r, v)| e.eval(r) == *v);
    // One draw times a number plus a number: the slope from the samples with that draw's lowest and highest values,
    // kept when the draw took enough different values (3, or all of a smaller draw's) and every sample fits.
    for (j, &n) in sizes.iter().enumerate() {
        let (lo, hi) = (samples.iter().min_by_key(|s| s.0[j])?, samples.iter().max_by_key(|s| s.0[j])?);
        let dx = hi.0[j] - lo.0[j];
        if dx == 0 || (hi.1 - lo.1) % dx != 0 {
            continue;
        }
        let a = (hi.1 - lo.1) / dx;
        let e = Expr::Lin(a, j, lo.1 - a * lo.0[j]);
        let seen: BTreeSet<i64> = samples.iter().map(|s| s.0[j]).collect();
        if a != 0 && seen.len() >= 3.min(n as usize) && check(&e) {
            return Some(e);
        }
    }
    // Two draws with small factors (±1, ±2), the constant from the first sample; tried only on 10 or more samples.
    for j in 0..nv {
        if samples.len() < 10 {
            break;
        }
        for k in j + 1..nv {
            for a in [1, -1, 2, -2] {
                for c in [1, -1, 2, -2] {
                    let b = samples[0].1 - a * samples[0].0[j] - c * samples[0].0[k];
                    let e = Expr::Lin2(a, j, c, k, b);
                    if check(&e) {
                        return Some(e);
                    }
                }
            }
        }
    }
    // A table indexed by a small draw: every value seen, each always giving the same number.
    for (j, &n) in sizes.iter().enumerate() {
        if !(1..=TABLE_MAX).contains(&n) {
            continue;
        }
        let mut table: Vec<Option<i64>> = vec![None; n as usize];
        let mut ok = true;
        for (r, v) in samples {
            let slot = &mut table[r[j] as usize];
            if slot.is_some_and(|t| t != *v) {
                ok = false;
                break;
            }
            *slot = Some(*v);
        }
        if ok && table.iter().all(Option::is_some) {
            return Some(Expr::Table(j, table.into_iter().flatten().collect()));
        }
    }
    None
}

/// A function's Lua, written while walking its runs token by token.
struct Writer {
    out: String,
    /// Each draw's size, by position.
    sizes: Vec<i64>,
    /// The map's x offset: a status-bar x is the game's x minus `pad`, plus 1 (y is plus 1).
    pad: i64,
    /// The functions the triggers it makes call.
    calls: BTreeSet<String>,
    /// Whether the Lua calls the `inside` helper (a rectangle loop).
    uses_inside: bool,
}

impl Writer {
    /// The Lua name of draw `i`: `r1`, `r2`...
    fn var(i: usize) -> String {
        format!("r{}", i + 1)
    }

    /// The names of the draws so far.
    fn vars(&self) -> Vec<String> {
        (0..self.sizes.len()).map(Self::var).collect()
    }

    /// The draws a token makes.
    fn draws(t: &Tok) -> usize {
        match t {
            Tok::Rand(_) => 1,
            Tok::Loop { sizes, .. } => sizes.len(),
            _ => 0,
        }
    }

    /// Writes `runs` from token `i` (where the draws so far number `nv`) at `indent`, leaving out each run's last
    /// `trim` tokens (written after the `if` they're common to).
    fn walk(&mut self, runs: &[&Run], mut i: usize, nv: usize, indent: usize, trim: usize) -> Result<(), String> {
        let pad = "  ".repeat(indent);
        let mut nv = nv;
        let view = |r: &Run| r.toks[..r.toks.len() - trim].to_vec();
        // Write tokens while every run has the same one; at the first difference, `split` takes over.
        loop {
            self.sizes.truncate(nv);
            let tok = view(runs[0]).get(i).cloned();
            if runs.iter().any(|r| view(r).get(i) != tok.as_ref()) {
                return self.split(runs, i, nv, indent, trim);
            }
            let Some(tok) = tok else { return Ok(()) };
            let tok = &tok;
            match tok {
                Tok::Rand(n) => {
                    let _ = writeln!(self.out, "{pad}local {} = game.rand({n})", Self::var(nv));
                    self.sizes.push(*n);
                }
                Tok::Loop { rect, sizes } => {
                    let first = nv;
                    let names: Vec<String> = (first..first + sizes.len()).map(Self::var).collect();
                    self.sizes.extend(sizes);
                    // The tile tested, from each try's own draws (shifted to sit where the loop's draws do).
                    let mut tries: Vec<Try> = Vec::new();
                    for r in runs {
                        for (d, at, inside) in &r.tries[&i] {
                            let mut full = vec![0; first];
                            full.extend(d);
                            tries.push((full, *at, *inside));
                        }
                    }
                    let xs: Vec<(&[i64], i64)> = tries.iter().map(|t| (t.0.as_slice(), t.1 .0)).collect();
                    let ys: Vec<(&[i64], i64)> = tries.iter().map(|t| (t.0.as_slice(), t.1 .1)).collect();
                    let (Some(fx), Some(fy)) = (fit(&xs, &self.sizes), fit(&ys, &self.sizes)) else {
                        return Err("its rectangle test's tile follows no rule".into());
                    };
                    let tested = |t: &Try| {
                        let (x, y) = (fx.eval(&t.0), fy.eval(&t.0));
                        x >= rect[0] && x <= rect[2] && y >= rect[1] && y <= rect[3]
                    };
                    if !tries.iter().all(|t| tested(t) == t.2) {
                        return Err("its rectangle test doesn't match a plain inside-the-corners test".into());
                    }
                    // The answer that makes it try again, from any try but a run's last; with only last tries, inside.
                    let again = runs.iter().flat_map(|r| r.tries[&i].iter().rev().skip(1)).map(|t| t.2).next().unwrap_or(true);
                    let vars = self.vars();
                    // Lua's tiles are status-bar tiles (x less the map's pad, both plus 1), as the rest of the export.
                    let (x, y) = (fx.clone().plus(1 - self.pad).lua(&vars), fy.clone().plus(1).lua(&vars));
                    let r = [rect[0] + 1 - self.pad, rect[1] + 1, rect[2] + 1 - self.pad, rect[3] + 1];
                    self.uses_inside = true;
                    let _ = writeln!(self.out, "{pad}local {}", names.join(", "));
                    let _ = writeln!(self.out, "{pad}repeat");
                    for (k, n) in names.iter().zip(sizes) {
                        let _ = writeln!(self.out, "{pad}  {k} = game.rand({n})");
                    }
                    let cond = if again { "not " } else { "" };
                    let _ = writeln!(self.out, "{pad}until {cond}inside({x}, {y}, {{ {}, {}, {}, {} }})", r[0], r[1], r[2], r[3]);
                }
                Tok::Effect(api) => {
                    let fits: Option<Vec<Expr>> = (0..runs[0].args[&i].len())
                        .map(|a| {
                            let samples: Vec<(&[i64], i64)> = runs.iter().map(|r| (&r.vals[..nv], r.args[&i][a])).collect();
                            fit(&samples, &self.sizes)
                        })
                        .collect();
                    match fits {
                        Some(f) => {
                            let line = self.effect(api, &f)?;
                            let _ = writeln!(self.out, "{pad}{line}");
                        }
                        // The arguments may follow a rule for each value of an earlier draw (a storm's side, say).
                        None => self.by_value(runs, i, nv, api, &pad)?,
                    }
                }
                Tok::Timer(..) | Tok::Made(..) | Tok::Destroy | Tok::Line(_) => self.plain(tok, &pad)?,
            }
            nv += Self::draws(tok);
            i += 1;
        }
    }

    /// A token without draws or varying numbers: a new time or count trigger, a message, or the end of the trigger
    /// that fired.
    fn plain(&mut self, tok: &Tok, pad: &str) -> Result<(), String> {
        match tok {
            Tok::Timer(args, name) => {
                self.calls.insert(name.clone());
                let fields = match args.as_slice() {
                    [enabled, one_shot, min, max] => timer_fields(*enabled != 0, *one_shot != 0, *min, Some(*max)),
                    [enabled, one_shot, time] => timer_fields(*enabled != 0, *one_shot != 0, *time, None),
                    _ => return Err(format!("CreateTimeTrigger({args:?})")),
                };
                let _ = writeln!(self.out, "{pad}trigger.time{{{fields}, on = {name}}}{}", timer_comment(args));
            }
            Tok::Made(lua, name) => {
                self.calls.insert(name.clone());
                let _ = writeln!(self.out, "{pad}{lua}");
            }
            Tok::Destroy => {
                let _ = writeln!(self.out, "{pad}t:destroy()");
            }
            Tok::Line(lua) => {
                let _ = writeln!(self.out, "{pad}{lua}");
            }
            _ => return Err("a draw where none was expected".into()),
        }
        Ok(())
    }

    /// Splits `runs`, which differ from token `i` on, by an earlier draw that decides how, and writes each part.
    fn split(&mut self, runs: &[&Run], i: usize, nv: usize, indent: usize, trim: usize) -> Result<(), String> {
        let pad = "  ".repeat(indent);
        let view = |r: &Run| r.toks[..r.toks.len() - trim].to_vec();
        let keys: Vec<Vec<Tok>> = runs.iter().map(|r| view(r)[i.min(view(r).len())..].to_vec()).collect();
        // New triggers, messages and the end of the old trigger that every part ends with go after the `if`.
        let mut tail = 0;
        while keys.iter().all(|k| k.len() > tail) {
            let t = &keys[0][keys[0].len() - 1 - tail];
            if !matches!(t, Tok::Timer(..) | Tok::Made(..) | Tok::Destroy | Tok::Line(_)) || keys.iter().any(|k| &k[k.len() - 1 - tail] != t) {
                break;
            }
            tail += 1;
        }
        // Try each earlier draw as the one that decides; draws of more than `TABLE_MAX * 8` values aren't tried.
        'var: for j in 0..nv {
            if self.sizes[j] > TABLE_MAX * 8 {
                continue;
            }
            // Group the draw's values by what they lead to; each value must always lead to the same.
            let mut groups: Vec<(usize, BTreeSet<i64>, Vec<&Run>)> = Vec::new();
            let mut lead: BTreeMap<i64, usize> = BTreeMap::new();
            for (r, k) in runs.iter().zip(&keys) {
                let v = r.vals[j];
                let g = match groups.iter().position(|(rep, _, _)| keys[*rep] == *k) {
                    Some(g) => g,
                    None => {
                        groups.push((keys.iter().position(|x| x == k).unwrap_or(0), BTreeSet::new(), Vec::new()));
                        groups.len() - 1
                    }
                };
                if *lead.entry(v).or_insert(g) != g {
                    continue 'var;
                }
                groups[g].1.insert(v);
                groups[g].2.push(r);
            }
            if groups.len() < 2 {
                // Every value leads the same way: the arguments themselves follow no rule.
                if keys.iter().all(|k| *k == keys[0]) {
                    break;
                }
                continue;
            }
            groups.sort_by_key(|g| *g.1.iter().next().unwrap_or(&0));
            let var = Self::var(j);
            let sizes = self.sizes.clone();
            // Branches that take the draw's values in bands (each one's below the next one's) are cut where the next
            // band starts, so a value the runs didn't happen to draw goes the way of its neighbours (Close
            // Encounter's Lava erupts below 40: `cmp eax, 0x28` at 0x1100412A).
            let low = |g: &BTreeSet<i64>| *g.iter().next().unwrap_or(&0);
            let high = |g: &BTreeSet<i64>| *g.iter().next_back().unwrap_or(&0);
            let banded = groups.windows(2).all(|w| high(&w[0].1) < low(&w[1].1));
            for (gi, (_, values, part)) in groups.iter().enumerate() {
                let head = if gi == 0 { "if" } else { "elseif" };
                let before = self.out.len();
                if gi + 1 == groups.len() {
                    let _ = writeln!(self.out, "{pad}else");
                } else if banded {
                    let _ = writeln!(self.out, "{pad}{head} {var} < {} then", low(&groups[gi + 1].1));
                } else {
                    let _ = writeln!(self.out, "{pad}{head} {} then", condition(&var, values));
                }
                self.sizes = sizes.clone();
                let head_len = self.out.len();
                self.walk(part, i, nv, indent + 1, trim + tail)?;
                // An `else` with nothing in it is left out.
                if gi + 1 == groups.len() && self.out.len() == head_len {
                    self.out.truncate(before);
                }
            }
            let _ = writeln!(self.out, "{pad}end");
            for t in &keys[0][keys[0].len() - tail..] {
                self.plain(t, &pad)?;
            }
            return Ok(());
        }
        Err("its runs differ in a way no one earlier draw decides".into())
    }

    /// Writes the effect at token `i` as an `if` on an earlier draw of few values, its arguments following a rule
    /// for each value (values with the same rules share a branch).
    fn by_value(&mut self, runs: &[&Run], i: usize, nv: usize, api: &str, pad: &str) -> Result<(), String> {
        'var: for j in 0..nv {
            if !(2..=TABLE_MAX).contains(&self.sizes[j]) {
                continue;
            }
            let mut parts: BTreeMap<i64, Vec<&Run>> = BTreeMap::new();
            for r in runs {
                parts.entry(r.vals[j]).or_default().push(r);
            }
            let mut rules: Vec<(BTreeSet<i64>, Vec<Expr>)> = Vec::new();
            for (v, part) in &parts {
                let mut f = Vec::new();
                for a in 0..part[0].args[&i].len() {
                    let samples: Vec<(&[i64], i64)> = part.iter().map(|r| (&r.vals[..nv], r.args[&i][a])).collect();
                    match fit(&samples, &self.sizes) {
                        Some(e) => f.push(e),
                        None => continue 'var,
                    }
                }
                match rules.iter_mut().find(|(_, g)| *g == f) {
                    Some((vals, _)) => {
                        vals.insert(*v);
                    }
                    None => rules.push((BTreeSet::from([*v]), f)),
                }
            }
            if rules.len() < 2 {
                continue;
            }
            let var = Self::var(j);
            for (k, (vals, f)) in rules.iter().enumerate() {
                let line = self.effect(api, f)?;
                if k + 1 == rules.len() {
                    let _ = writeln!(self.out, "{pad}else");
                } else {
                    let _ = writeln!(self.out, "{pad}{} {} then", if k == 0 { "if" } else { "elseif" }, condition(&var, vals));
                }
                let _ = writeln!(self.out, "{pad}  {line}");
            }
            let _ = writeln!(self.out, "{pad}end");
            return Ok(());
        }
        Err(format!("the arguments of its {} follow no rule", api.trim_start_matches("TethysGame::")))
    }

    /// The Lua line of a [`DISASTERS`] call whose numbers are `f`, tiles in status-bar coordinates (a target of -1, -1
    /// is `nil`).
    fn effect(&self, api: &str, f: &[Expr]) -> Result<String, String> {
        let vars = self.vars();
        let x = |e: &Expr| e.clone().plus(1 - self.pad).lua(&vars);
        let y = |e: &Expr| e.clone().plus(1).lua(&vars);
        let tile = |i: usize| format!("{{ {}, {} }}", x(&f[i]), y(&f[i + 1]));
        let to = |i: usize| if f[i] == Expr::Const(-1) && f[i + 1] == Expr::Const(-1) { "nil".to_string() } else { tile(i) };
        let n = |i: usize| f[i].lua(&vars);
        let want = |k: usize| if f.len() == k { Ok(()) } else { Err(format!("{api} with {} numbers", f.len())) };
        Ok(match api {
            "TethysGame::SetMeteor" => {
                want(3)?;
                format!("game.create_meteor({}, {})", tile(0), n(2))
            }
            "TethysGame::SetEarthquake" => {
                want(3)?;
                format!("game.create_earthquake({}, {})", tile(0), n(2))
            }
            "TethysGame::SetEruption" => {
                want(3)?;
                format!("game.create_eruption({}, {})", tile(0), n(2))
            }
            "TethysGame::SetLightning" => {
                want(5)?;
                format!("game.create_lightning({}, {}, {})", tile(0), to(3), n(2))
            }
            "TethysGame::SetTornado" => {
                want(6)?;
                let now = match &f[5] {
                    Expr::Const(0) => String::new(),
                    Expr::Const(_) => ", true".into(),
                    _ => return Err("a vortex that's sometimes immediate".into()),
                };
                format!("game.create_tornado({}, {}, {}{now})", tile(0), to(3), n(2))
            }
            "GameMap::SetTile" => {
                want(3)?;
                format!("game.set_tile({}, {})", tile(0), n(2))
            }
            "GameMap::InitialSetTile" => {
                want(3)?;
                format!("game.initial_set_tile({}, {})", tile(0), n(2))
            }
            "GameMap::SetCellType" => {
                want(3)?;
                format!("game.set_cell_type({}, {})", tile(0), n(2))
            }
            // The Blight on a tile (Eden 2's StartMicrobe), or off it.
            "GameMap::SetVirusUL" => {
                want(3)?;
                match &f[2] {
                    Expr::Const(0) => format!("game.unset_blight({})", tile(0)),
                    Expr::Const(_) => format!("game.create_blight({})", tile(0)),
                    _ => return Err("SetVirusUL with a drawn value".into()),
                }
            }
            "TethysGame::SetMicrobeSpreadSpeed" => {
                want(1)?;
                format!("game.set_blight_speed({})", n(0))
            }
            "GameMap::SetLavaPossible" => {
                want(3)?;
                match &f[2] {
                    Expr::Const(0) => format!("game.set_lava_possible({}, false)", tile(0)),
                    Expr::Const(_) => format!("game.set_lava_possible({})", tile(0)),
                    _ => return Err("SetLavaPossible with a drawn value".into()),
                }
            }
            other => return Err(format!("it calls {other}")),
        })
    }
}

/// The Lua with each run of `game.set_lava_possible({ x, y })` lines (tiles marked where lava may flow, which draw
/// nothing, so their order doesn't matter) written as one loop over the rectangles that cover those tiles; runs of
/// fewer than 4 tiles stay lines.
fn compact_lava(lua: &str) -> String {
    let tile = |l: &str| -> Option<(String, i32, i32)> {
        let body = l.trim_start();
        let indent = l[..l.len() - body.len()].to_string();
        let (x, y) = body.strip_prefix("game.set_lava_possible({ ")?.strip_suffix(" })")?.split_once(", ")?;
        Some((indent, x.parse().ok()?, y.parse().ok()?))
    };
    let mut out = String::new();
    let lines: Vec<&str> = lua.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let Some((indent, ..)) = tile(lines[i]) else {
            out.push_str(lines[i]);
            out.push('\n');
            i += 1;
            continue;
        };
        let mut tiles: BTreeSet<(i32, i32)> = BTreeSet::new();
        while let Some((ind, x, y)) = lines.get(i).and_then(|l| tile(l)) {
            if ind != indent {
                break;
            }
            tiles.insert((y, x));
            i += 1;
        }
        if tiles.len() < 4 {
            for (y, x) in &tiles {
                let _ = writeln!(out, "{indent}game.set_lava_possible({{ {x}, {y} }})");
            }
            continue;
        }
        // Runs along each row, then runs of the same span on following rows joined into one rectangle.
        let mut spans: BTreeMap<(i32, i32), Vec<i32>> = BTreeMap::new();
        let mut row: Option<(i32, i32, i32)> = None;
        for &(y, x) in &tiles {
            row = match row {
                Some((ry, x1, x2)) if ry == y && x == x2 + 1 => Some((ry, x1, x)),
                Some((ry, x1, x2)) => {
                    spans.entry((x1, x2)).or_default().push(ry);
                    Some((y, x, x))
                }
                None => Some((y, x, x)),
            };
        }
        if let Some((ry, x1, x2)) = row {
            spans.entry((x1, x2)).or_default().push(ry);
        }
        let mut rects = Vec::new();
        for ((x1, x2), ys) in spans {
            let mut start = ys[0];
            for w in ys.windows(2) {
                if w[1] != w[0] + 1 {
                    rects.push([x1, start, x2, w[0]]);
                    start = w[1];
                }
            }
            rects.push([x1, start, x2, *ys.last().unwrap_or(&start)]);
        }
        rects.sort_by_key(|r| (r[1], r[0]));
        let _ = writeln!(out, "{indent}for _, r in ipairs({{");
        for c in rects.chunks(6) {
            let row: Vec<String> = c.iter().map(|r| format!("{{ {}, {}, {}, {} }}", r[0], r[1], r[2], r[3])).collect();
            let _ = writeln!(out, "{indent}  {},", row.join(", "));
        }
        let _ = writeln!(out, "{indent}}}) do game.set_lava_possible(r) end");
    }
    out
}

/// `r == 1`, `r >= 2 and r <= 5`, `r == 1 or r == 4`.
fn condition(var: &str, values: &BTreeSet<i64>) -> String {
    let (lo, hi) = (*values.iter().next().unwrap_or(&0), *values.iter().next_back().unwrap_or(&0));
    if lo == hi {
        format!("{var} == {lo}")
    } else if (hi - lo + 1) as usize == values.len() {
        format!("{var} >= {lo} and {var} <= {hi}")
    } else {
        values.iter().map(|v| format!("{var} == {v}")).collect::<Vec<_>>().join(" or ")
    }
}

/// `trigger.time`'s fields for a time trigger's numbers (OP2Lua's defaults: enabled, one-shot).
pub(crate) fn timer_fields(enabled: bool, one_shot: bool, min: i64, max: Option<i64>) -> String {
    let mut f = match max {
        Some(max) => vec![format!("min = {min}"), format!("max = {max}")],
        None => vec![format!("time = {min}")],
    };
    if !one_shot {
        f.push("one_shot = false".into());
    }
    if !enabled {
        f.push("enabled = false".into());
    }
    f.join(", ")
}

/// A Lua comment with a time trigger's wait in marks (its numbers as `CreateTimeTrigger` takes them, in ticks).
fn timer_comment(args: &[i64]) -> String {
    match args {
        [_, _, min, max] => format!("  -- after {} to {} marks", min / 100, max / 100),
        [_, _, t] => format!("  -- after {} marks", t / 100),
        _ => String::new(),
    }
}

/// A converted function: its body, whether it uses `inside`, and the functions the triggers it makes call.
struct Made {
    /// The function's body, indented for inside `function name(t) ... end`.
    lua: String,
    /// Whether the body calls the `inside` helper.
    inside: bool,
    /// The functions the triggers it makes call, to convert in turn.
    calls: BTreeSet<String>,
    /// It goes quiet for good after the call that makes its effects (Close Encounter's `Lava`, which draws each
    /// call until it erupts and then sets a flag at 0x110080B8).
    stops: bool,
}

/// `lua` with `<name>_done = true` after each run of effect lines: the function's flag that it has acted.
fn done_after_effects(lua: &str, name: &str) -> String {
    let effect = |l: &str| ["game.create_", "game.set_tile", "game.set_lava_possible", "game.message_at", "game.set_blight_speed", "game.unset_blight"].iter().any(|p| l.trim_start().starts_with(p));
    let lines: Vec<&str> = lua.lines().collect();
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        let _ = writeln!(out, "{l}");
        if effect(l) && !lines.get(i + 1).is_some_and(|n| effect(n)) {
            let indent = &l[..l.len() - l.trim_start().len()];
            let _ = writeln!(out, "{indent}{name}_done = true");
        }
    }
    out
}

/// A call that only reads the game: the lobby, a player's colony, the units (it makes and changes nothing).
fn is_read(api: &str) -> bool {
    let (class, method) = api.split_once("::").unwrap_or(("", api));
    match class {
        "TethysGame" => matches!(method, "NoPlayers" | "Tick" | "Time" | "UsesDayNight" | "UsesMorale" | "CanHaveDisasters" | "InitialUnits"),
        "_Player" => !["Set", "Go", "Mark", "Ally", "Center", "Capture", "Reset", "Force", "Free", "Do"].iter().any(|p| method.starts_with(p)),
        "PlayerVehicleEnum" | "PlayerBuildingEnum" | "PlayerUnitEnum" | "GroupEnumerator" | "InRectEnumerator" | "LocationEnumerator" => true,
        "Unit" => method.starts_with("Get") || method.starts_with("Is") || method.starts_with("Has"),
        _ => false,
    }
}

/// The calls with which a function reads the colonies' size: the player count, then each player's kids, workers and
/// scientists.
const POPULATION: [&str; 4] = ["TethysGame::NoPlayers", "_Player::Kids", "_Player::Workers", "_Player::Scientists"];

/// A function that adds up every player's kids, workers and scientists and acts once the total reaches a number
/// (Biohazard's `LavaCheck` erupts at 200, `cmp ebx, 0xC8` in the function at 0x110050E0). The number is found by
/// setting the population and searching for the smallest total that makes it act; then the same total must make it
/// act however it's split between players and kinds, one fewer must not, and its effects must be fixed calls.
fn population(dll: &Path, opts: &op2_missionhost::runtime::Options, pad: i32, name: &str) -> Result<Made, String> {
    let call = |people: &[[i32; 3]]| -> Result<[Vec<CallRec>; 2], String> {
        let runs = crate::calls::with_population(dll, opts, name, people).map_err(|e| e.to_string())?;
        Ok(runs.map(|r| r.into_iter().filter(|c| !POPULATION.contains(&c.api.as_str()) && c.api != "ScStub::~ScStub").collect()))
    };
    let acts = |people: &[[i32; 3]]| call(people).map(|[first, _]| !first.is_empty());
    // People as one player's [kids, workers, scientists]; `call` gives two calls in a row, `acts` whether the first did
    // anything.
    let workers = |n: i32| vec![[0, n, 0]];
    // Binary search for the smallest total of workers that makes it act, between none and 2^20.
    let (mut lo, mut hi) = (0, 1 << 20);
    if acts(&workers(lo))? || !acts(&workers(hi))? {
        return Err("it reads the colonies' size but doesn't act on a total of it".into());
    }
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if acts(&workers(mid))? { hi = mid } else { lo = mid }
    }
    let n = hi;
    // The same total as kids alone, scientists alone and split over players and kinds must act; one fewer must not.
    let (a, b) = (n / 3, n - n / 3);
    for (people, should) in [
        (vec![[n, 0, 0]], true),
        (vec![[0, 0, n]], true),
        (vec![[a, 0, 0], [0, 0, 0], [0, b, 0]], true),
        (vec![[n - 1, 0, 0]], false),
        (vec![[0, 0, a], [0, b - 1, 0]], false),
    ] {
        if acts(&people)? != should {
            return Err("it reads the colonies' size, but not as one total of every player's people".into());
        }
    }
    let [effects, again] = call(&workers(n))?;
    let mut w = Writer { out: String::new(), sizes: Vec::new(), pad: pad as i64, calls: BTreeSet::new(), uses_inside: false };
    let mut body = String::new();
    for it in items(&effects)? {
        let Item::Effect(api, args) = it else { return Err("it does more than fixed calls once the colonies are big enough".into()) };
        let f: Vec<Expr> = args.into_iter().map(Expr::Const).collect();
        let _ = writeln!(body, "    {}", w.effect(api, &f)?);
    }
    let _ = writeln!(w.out, "  local total = 0");
    let _ = writeln!(w.out, "  for id = 1, game.num_players() do total = total + players[id].population end");
    let _ = writeln!(w.out, "  if total >= {n} then");
    w.out.push_str(&body);
    let _ = writeln!(w.out, "  end");
    // Acting once and then no more is the flag Biohazard sets at 0x110093B8.
    let stops = again.is_empty();
    let lua = if stops { done_after_effects(&w.out, name) } else { w.out };
    Ok(Made { lua, inside: false, calls: w.calls, stops })
}

/// Games sampled for a function that goes quiet once it has acted.
const STOP_GAMES: usize = 200;
/// Calls of the function made in each of the [`STOP_GAMES`] games.
const STOP_CALLS: usize = 20;

/// The Lua function `name` of the mission `dll`, from `RUNS` runs of it, or why it can't be made. `players` is the
/// lobby's player count, or 0 for a single-player mission. A single-player mission's function that asks the player's
/// difficulty (`_Player::Difficulty`, seen in 20 probe calls) is made at each difficulty, and the Lua picks the body by
/// `players[1].difficulty` (one body when all three are the same).
fn function(dll: &Path, players: u8, pad: i32, name: &str, mut ai: Option<&mut crate::ai::Ai>) -> Result<Made, String> {
    if players != 0 {
        return function_with(dll, &crate::calls::disasters_on(players, 1), pad, name, ai, false, true);
    }
    // A single-player mission (no players given) runs without a lobby.
    let at = |difficulty: i32| op2_missionhost::runtime::Options {
        call_log: true,
        difficulty,
        ..op2_missionhost::runtime::Options::from_env()
    };
    let probe = crate::calls::repeat(dll, &at(1), name, 20).map_err(|e| e.to_string())?;
    if !probe.iter().flatten().any(|c| c.api == "_Player::Difficulty") {
        return function_with(dll, &at(1), pad, name, ai, false, false);
    }
    let mut made = Vec::new();
    for d in 0..3 {
        made.push(function_with(dll, &at(d), pad, name, ai.as_deref_mut(), true, false)?);
    }
    if made.iter().any(|m| m.stops != made[0].stops) {
        return Err("it stops at some difficulties only".into());
    }
    let calls = made.iter().flat_map(|m| m.calls.iter().cloned()).collect();
    let inside = made.iter().any(|m| m.inside);
    let stops = made[0].stops;
    if made.iter().all(|m| m.lua == made[0].lua) {
        return Ok(Made { lua: made.swap_remove(0).lua, inside, calls, stops });
    }
    let indent = |lua: &str| lua.lines().map(|l| if l.is_empty() { String::new() } else { format!("  {l}\n") }).collect::<String>();
    let mut lua = String::new();
    for (d, m) in made.iter().enumerate() {
        let keyword = if d == 0 { "if" } else { "elseif" };
        if d < 2 {
            let _ = writeln!(lua, "  {keyword} players[1].difficulty == \"{}\" then", crate::luaexport::LEVEL_NAMES[d]);
        } else {
            let _ = writeln!(lua, "  else");
        }
        lua.push_str(&indent(&m.lua));
    }
    let _ = writeln!(lua, "  end");
    Ok(Made { lua, inside, calls, stops })
}

/// [`function`] with the game's options `opts`. `ai` converts a function of the computer player's groups;
/// `by_difficulty` leaves out the function's reads of the difficulty (the caller runs it at each); `drop_reads`
/// (multiplayer) leaves out the reads of the game after each run's last action, with a Lua comment saying so.
fn function_with(
    dll: &Path,
    opts: &op2_missionhost::runtime::Options,
    pad: i32,
    name: &str,
    ai: Option<&mut crate::ai::Ai>,
    by_difficulty: bool,
    drop_reads: bool,
) -> Result<Made, String> {
    let clean = |r: Vec<CallRec>| -> Vec<CallRec> { r.into_iter().filter(|c| !by_difficulty || c.api != "_Player::Difficulty").collect() };
    // A function of the computer player's groups (ai.rs): a few calls show its steps.
    if let Some(ai) = ai {
        let runs = crate::calls::repeat(dll, opts, name, crate::ai::RUNS).map_err(|e| e.to_string())?;
        if runs.iter().any(|r| crate::ai::uses_groups(r)) {
            let (lua, calls) = crate::ai::function(ai, name, &runs)?;
            return Ok(Made { lua, inside: false, calls, stops: false });
        }
    }
    let mut runs: Vec<Vec<CallRec>> = crate::calls::repeat(dll, opts, name, RUNS).map_err(|e| e.to_string())?.into_iter().map(clean).collect();
    // In a multiplayer mission (`drop_reads`), the reads of the game after a run's last action (Biohazard's Quake looks
    // over each player's colony after the quake), which led to nothing in that run: left out, and the Lua says so. Only
    // when some run acts; a function that only reads is left whole for the population rule below.
    let mut dropped_reads = false;
    if drop_reads && runs.iter().flatten().any(|c| !is_read(&c.api) && c.api != "ScStub::~ScStub") {
        for r in &mut runs {
            let keep = r.iter().rposition(|c| !is_read(&c.api) && c.api != "ScStub::~ScStub").map_or(0, |k| k + 1);
            dropped_reads |= r[keep..].iter().any(|c| is_read(&c.api));
            r.truncate(keep);
        }
    }
    // Only reads of the colonies' size (and the effects they lead to, which these runs don't reach): a population rule.
    let calls: Vec<&CallRec> = runs.iter().flatten().filter(|c| c.api != "ScStub::~ScStub").collect();
    if calls.iter().any(|c| POPULATION[1..].contains(&c.api.as_str()))
        && calls.iter().all(|c| POPULATION.contains(&c.api.as_str()) || DISASTERS.contains(&c.api.as_str()))
    {
        return population(dll, opts, pad, name);
    }
    // A function that goes quiet for good after some call: one game shows it act once, so the calls of many games,
    // each up to the call it acted in. Only that call may have effects, and every call before it something.
    // It stops when the last runs call nothing.
    let stops = runs.iter().rposition(|r| !r.is_empty()).is_some_and(|k| k + 1 < runs.len());
    let mut last = Vec::new();
    if stops {
        runs.clear();
        for game in crate::calls::games_of(dll, opts, name, STOP_GAMES, STOP_CALLS).map_err(|e| e.to_string())? {
            let Some(k) = game.iter().rposition(|r| !r.is_empty()) else { continue };
            if game[..k].iter().any(Vec::is_empty) {
                return Err("it does nothing on some calls and acts again later".into());
            }
            last.extend((0..=k).map(|i| i == k && k + 1 < game.len()));
            runs.extend(game.into_iter().take(k + 1).map(clean));
        }
    }
    let items: Vec<Vec<Item>> = runs.iter().map(|r| items(r)).collect::<Result<_, _>>()?;
    if stops {
        for (i, its) in items.iter().enumerate() {
            if its.iter().any(|it| matches!(it, Item::Effect(..) | Item::Line(_))) != last[i] {
                return Err("it goes quiet after a call that acts, but not the way converted (its effects, then nothing)".into());
            }
        }
    }
    if items.iter().all(Vec::is_empty) {
        return Err("it calls nothing".into());
    }
    // `t:destroy()` stands for ending the trigger that fired: the one the run before made, when it made one.
    for w in items.windows(2) {
        let made = w[0].iter().find_map(|i| if let Item::Timer(_, _, id) = i { Some(*id) } else { None });
        let ended = w[1].iter().find_map(|i| if let Item::Destroy(id) = i { Some(*id) } else { None });
        if let (Some(made), Some(ended)) = (made, ended)
            && made != ended
        {
            return Err("it ends a trigger other than the one that fired".into());
        }
    }
    // A rectangle test is redone while its answer is `again`: whichever reading leaves fewer different runs.
    let size = try_sizes(&items);
    let read = |again: bool| -> Vec<Run> { items.iter().map(|i| tokens(i, again, &size)).collect() };
    let shapes = |runs: &[Run]| runs.iter().map(|r| format!("{:?}", r.toks)).collect::<BTreeSet<_>>().len();
    let (inside_again, outside_again) = (read(true), read(false));
    let runs = if shapes(&outside_again) < shapes(&inside_again) { outside_again } else { inside_again };
    let refs: Vec<&Run> = runs.iter().collect();
    let mut w = Writer { out: String::new(), sizes: Vec::new(), pad: pad as i64, calls: BTreeSet::new(), uses_inside: false };
    w.walk(&refs, 0, 0, 1, 0)?;
    if dropped_reads {
        let _ = writeln!(w.out, "  -- The original then reads the game (each player's colony, units), which led to nothing in {RUNS} calls: not converted.");
    }
    let lua = if stops { done_after_effects(&w.out, name) } else { w.out };
    Ok(Made { lua, inside: w.uses_inside, calls: w.calls, stops })
}

/// The functions the setup's `triggers` call (those not in `skip`), converted where they follow the rules above, and
/// the triggers that call them; a function that isn't converted is made empty and its triggers still made, with the
/// reason in `notes`. `gated` triggers are made only when the lobby allows disasters. `callbacks` are the functions
/// the win's triggers call (victory.rs makes those triggers), converted or made empty here too.
/// `ai`, the computer player, converts the functions that work on its groups, and the triggers that watch it.
/// `players` is the lobby's number of players, or 0 for a single-player mission.
#[allow(clippy::too_many_arguments)]
pub fn convert(
    dll: &Path,
    players: u8,
    pad: i32,
    triggers: &[Trigger],
    gated: &BTreeSet<u32>,
    skip: &BTreeSet<u32>,
    callbacks: &BTreeSet<String>,
    mut ai: Option<&mut crate::ai::Ai>,
    notes: &mut Vec<String>,
) -> Timed {
    let mut timed = Timed {
        functions: String::new(),
        init: String::new(),
        init_by_id: Vec::new(),
        handled: BTreeSet::new(),
        converted: BTreeSet::new(),
        stubs: BTreeSet::new(),
    };
    let wanted = wanted_of(triggers, skip);
    if wanted.is_empty() && callbacks.is_empty() {
        return timed;
    }
    // Every function a trigger names (the win's triggers' too: `callbacks`), then those the triggers its functions
    // make name, until no more turn up.
    let mut made: BTreeMap<String, Result<Made, String>> = BTreeMap::new();
    let mut todo: Vec<String> = wanted.iter().filter_map(|t| t.callback.clone()).chain(callbacks.iter().cloned()).collect();
    while let Some(n) = todo.pop() {
        if made.contains_key(&n) {
            continue;
        }
        let m = function(dll, players, pad, &n, ai.as_deref_mut());
        if let Ok(m) = &m {
            todo.extend(m.calls.iter().cloned());
        }
        made.insert(n, m);
    }
    let ok: BTreeMap<&String, &Made> = made.iter().filter_map(|(n, r)| r.as_ref().ok().map(|v| (n, v))).collect();
    for (n, r) in &made {
        if let Err(why) = r {
            notes.push(format!("{n} isn't converted: {why}"));
        }
    }
    // Triggers whose function isn't converted are still made, calling an empty function: the game draws a random time
    // as it makes one (a colony game's disasters come in windows; Biohazard's Quake), so every later draw and trigger
    // stays as the original's. A function that makes a trigger calling one of them makes it the same way (CES1's
    // SetupBase2 starts Base2CommonCheck).
    let stubs: BTreeSet<&String> = {
        let called = ok.values().flat_map(|m| m.calls.iter());
        wanted.iter().filter_map(|t| t.callback.as_ref()).chain(called).chain(callbacks.iter()).filter(|c| matches!(made.get(*c), Some(Err(_)))).collect()
    };
    if ok.is_empty() && stubs.is_empty() {
        return timed;
    }
    timed.converted = ok.keys().map(|n| (*n).clone()).collect();
    timed.stubs = stubs.iter().map(|n| (*n).clone()).collect();
    let ai_player = ai.as_ref().map(|a| a.player);
    let f = &mut timed.functions;
    if !stubs.is_empty() {
        let _ = writeln!(f, "-- Not converted (the list at the end says why); their triggers are made as the original makes them.");
        for n in &stubs {
            let _ = writeln!(f, "function {n}(t) end");
        }
        f.push('\n');
    }
    if ok.is_empty() {
        return finish_triggers(timed, &wanted, gated, players, ai_player, notes);
    }
    let _ = writeln!(f, "-- Disasters and timed events: what the mission's trigger functions did over {RUNS} calls each, with");
    let _ = writeln!(f, "-- the same draws of the game's random numbers. Each is called with the trigger that fired.");
    if ok.values().any(|m| m.inside) {
        let _ = writeln!(f, "local function inside(x, y, r) return x >= r[1] and x <= r[3] and y >= r[2] and y <= r[4] end");
    }
    let _ = writeln!(f, "local {}", ok.keys().map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
    for (n, m) in &ok {
        if m.stops {
            let _ = writeln!(f, "\nlocal {n}_done  -- {n} does nothing more once it has acted");
            let _ = writeln!(f, "function {n}(t)");
            let _ = writeln!(f, "  if {n}_done then return end");
        } else {
            let _ = writeln!(f, "\nfunction {n}(t)");
        }
        f.push_str(&m.lua);
        let _ = writeln!(f, "end");
    }
    f.push('\n');
    timed.functions = compact_lava(&timed.functions);
    finish_triggers(timed, &wanted, gated, players, ai_player, notes)
}

/// The setup's triggers that call a function (not `NO_RESPONSE`), leaving out those in `skip` (the win's) and
/// objectives.
fn wanted_of<'a>(triggers: &'a [Trigger], skip: &BTreeSet<u32>) -> Vec<&'a Trigger> {
    triggers
        .iter()
        .filter(|t| !skip.contains(&t.id) && t.objective.is_none())
        .filter(|t| t.callback.as_deref().is_some_and(|c| !crate::victory::NO_RESPONSE.contains(&c)))
        .collect()
}

/// The functions the setup's `triggers` call that `convert` would convert or make empty (those not in `skip`).
pub fn callbacks_of(triggers: &[Trigger], skip: &BTreeSet<u32>) -> BTreeSet<String> {
    wanted_of(triggers, skip).iter().filter_map(|t| t.callback.clone()).collect()
}

/// A single-player setup's triggers (another difficulty's) that call the functions `timed` converted or stubbed: the
/// lines for `on_init` (`init_by_id`) and the triggers covered, as `convert` makes them.
pub fn lines_for(timed: &Timed, triggers: &[Trigger], skip: &BTreeSet<u32>, ai_player: Option<i32>, notes: &mut Vec<String>) -> Timed {
    let out = Timed {
        functions: String::new(),
        init: String::new(),
        init_by_id: Vec::new(),
        handled: BTreeSet::new(),
        converted: timed.converted.clone(),
        stubs: timed.stubs.clone(),
    };
    finish_triggers(out, &wanted_of(triggers, skip), &BTreeSet::new(), 0, ai_player, notes)
}

/// `timed` with the Lua of the `wanted` triggers, in the setup's order, that call a converted function or a stub:
/// `init` (those made only when the lobby allows disasters, `gated`, together under one `if`), `init_by_id` (the
/// rest, one by one) and `handled` (not those that call a stub). `players` is the lobby's count, for the per-player
/// loops (0: none); `ai_player` the computer player, for triggers that watch it.
#[allow(clippy::too_many_arguments)]
fn finish_triggers(
    mut timed: Timed,
    wanted: &[&Trigger],
    gated: &BTreeSet<u32>,
    players: u8,
    ai_player: Option<i32>,
    notes: &mut Vec<String>,
) -> Timed {
    let (ok, stubs) = (timed.converted.clone(), timed.stubs.clone());
    let mut always = String::new();
    let mut lobby = String::new();
    // A trigger made once for each player alike (Close Encounter's ten Lynxes, a count trigger per player that
    // calls Lava): one loop over the game's players, written at the first of them.
    let per_player = |t: &Trigger, name: &str| crate::victory::constructor(t, Some("id"), &BTreeMap::new(), &format!("on = {name}")).ok();
    let mut in_loop: BTreeSet<u32> = BTreeSet::new();
    for t in wanted {
        let Some(name) = t.callback.as_ref().filter(|c| ok.contains(*c) || stubs.contains(*c)) else { continue };
        let stub = stubs.contains(name);
        if in_loop.contains(&t.id) {
            continue;
        }
        if let Some(c) = per_player(t, name).filter(|_| crate::victory::constructor(t, None, &BTreeMap::new(), "").is_err()) {
            let alike: Vec<u32> = wanted
                .iter()
                .filter(|o| o.callback.as_deref() == Some(name.as_str()) && per_player(o, name).as_deref() == Some(c.as_str()))
                .map(|o| o.id)
                .collect();
            if alike.len() == players as usize && alike.iter().all(|id| gated.contains(id) == gated.contains(&t.id)) {
                in_loop.extend(&alike);
                // As below: triggers calling a stub stay uncovered, so their notes stay.
                if !stub {
                    timed.handled.extend(&alike);
                }
                let (into, pad) = if gated.contains(&t.id) { (&mut lobby, "    ") } else { (&mut always, "  ") };
                let mark = if stub { format!("  -- {name} isn't converted") } else { String::new() };
                let _ = writeln!(into, "{pad}for id = 1, n do {c} end{mark}");
                continue;
            }
        }
        let line = match &t.spec {
            TriggerSpec::Time { min, max, random } => {
                let (min, max) = (*min as i64, *max as i64);
                let (top, args) = if *random { (Some(max), vec![0, 0, min, max]) } else { (None, vec![0, 0, min]) };
                format!("trigger.time{{{}, on = {name}}}{}", timer_fields(t.enabled, t.one_shot, min, top), timer_comment(&args))
            }
            _ => match crate::victory::constructor(t, None, &BTreeMap::new(), &format!("on = {name}")).or_else(|why| match ai_player {
                // A trigger that watches the computer player (its smelter built, say).
                Some(p) if why == format!("it watches player {} only", p + 1) => {
                    crate::victory::constructor(t, Some("ai_player"), &BTreeMap::new(), &format!("on = {name}"))
                }
                // A trigger on one player by number (River of Fire's first mine is player 1's): Lua's
                // `players[n]` is the game's player n - 1, as in the rest of the conversion.
                _ => match why.strip_prefix("it watches player ").and_then(|r| r.strip_suffix(" only")) {
                    Some(n) => crate::victory::constructor(t, Some(n), &BTreeMap::new(), &format!("on = {name}")),
                    None => Err(why),
                },
            }) {
                Ok(c) => c,
                Err(why) => {
                    notes.push(format!("trigger #{} calls {name}, but isn't converted: {why}", t.id));
                    continue;
                }
            },
        };
        // A trigger that calls a stub isn't covered: the reason its function isn't converted stays in the notes.
        if !stub {
            timed.handled.insert(t.id);
        }
        let line = if stub { format!("{line}  -- {name} isn't converted") } else { line };
        if !gated.contains(&t.id) {
            timed.init_by_id.push((t.id, format!("  {line}\n")));
        }
        let (into, pad) = if gated.contains(&t.id) { (&mut lobby, "    ") } else { (&mut always, "  ") };
        let _ = writeln!(into, "{pad}{line}");
    }
    timed.init.push_str(&always);
    if !lobby.is_empty() {
        let _ = writeln!(timed.init, "  if game.disasters_enabled() then");
        timed.init.push_str(&lobby);
        let _ = writeln!(timed.init, "  end");
    }
    timed
}
