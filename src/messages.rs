//! The messages a mission posts to the players ("Your scouts report..."): every text its code passes to
//! `TethysGame::AddMessage`, read from the DLL with the exported functions that reach each call and the triggers that
//! call those, and a replay with nobody playing, as the disaster forecast is, for when they appear. The replay is a
//! prediction: messages that wait on the player never come, and some depend on the game's random numbers.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Result;
use iced_x86::{Decoder, DecoderOptions, FlowControl, OpKind};
use op2_missionhost::play::Play;
use op2_missionhost::runtime::Options;
use op2_x86::pe::{ImportName, PeFile};
use serde::Serialize;

use crate::disasters::MARK;
use crate::setup::StartOptions;
use crate::triggers::{self, Trigger};

/// What a replay with nobody playing posted ([`replay`]): its messages and the triggers it saw made.
#[derive(Debug, Clone, Serialize)]
pub struct Messages {
    /// How far the replay got, in ticks.
    pub reached: u32,
    /// How far it was asked to go, in ticks.
    pub horizon: u32,
    /// Why the replay stopped early, or how the mission ended in it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
    /// The messages posted, in the order they were posted.
    pub messages: Vec<Message>,
    /// The triggers the setup made and those the replay saw made later.
    pub triggers: Vec<Made>,
}

/// One message posted in the replay.
#[derive(Debug, Clone, Serialize)]
pub struct Message {
    /// The tick it was posted (0: by the setup).
    pub tick: u32,
    pub text: String,
    /// The player it's for; -1 is everyone.
    pub to: i32,
    /// The sound it plays (a sound id; 0 none).
    pub sound: i32,
    /// The file tile it points at, if any (the game's jump-to location).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<(i32, i32)>,
    /// The mission function that posted it (`InitProc` for the setup).
    pub from: String,
}

/// A trigger and the tick it was made (0: by the setup).
#[derive(Debug, Clone, Serialize)]
pub struct Made {
    pub tick: u32,
    pub trigger: Trigger,
}

impl Message {
    /// "start" for the setup, else "mark N".
    pub fn when(&self) -> String {
        if self.tick == 0 { "start".into() } else { format!("mark {}", self.tick / MARK) }
    }

    /// "everyone" or "player N".
    pub fn to_text(&self) -> String {
        if self.to < 0 { "everyone".into() } else { format!("player {}", self.to) }
    }
}

/// Replays the mission at `start` for `marks` marks and collects the messages it posts and the triggers it makes.
pub fn replay(dll: &Path, start: StartOptions, marks: u32) -> Result<Messages> {
    let horizon = marks * MARK;
    let opts = Options { seed: 0, difficulty: start.difficulty, players: start.players, ..Options::from_env() };
    let mut play = Play::new(dll, "", &opts)?;
    // The tick each message was posted, by its index in the world's message list (0: by the setup).
    let mut posted: Vec<u32> = vec![0; play.world().settings.messages.len()];
    let techs = crate::setup::tech_names(dll, &play.ms.desc.techtree_name);
    let mut made: Vec<Made> = triggers::collect(play.world(), &[], &techs).into_iter().map(|trigger| Made { tick: 0, trigger }).collect();
    let mut stubs = play.world().stubs.len();
    let mut stopped = None;
    while play.tick() < horizon {
        if let Some((end, tick)) = play.world().end {
            stopped = Some(format!("the mission ended ({end:?}) at mark {}", tick / MARK));
            break;
        }
        if let Err(e) = play.step() {
            stopped = Some(format!("{e:#}"));
            break;
        }
        // The messages this step added were posted at this tick.
        posted.resize(play.world().settings.messages.len(), play.tick());
        // New triggers: read them before a one-shot one fires and is gone.
        if play.world().stubs.len() != stubs {
            stubs = play.world().stubs.len();
            for trigger in triggers::collect(play.world(), &[], &techs) {
                if !made.iter().any(|m| m.trigger.id == trigger.id) {
                    made.push(Made { tick: play.tick(), trigger });
                }
            }
        }
    }
    let w = play.world();
    // The calls that posted them, in the same order: who posted each and where it points.
    let calls: Vec<_> = play.ms.rt.log.iter().filter(|c| c.api.ends_with("::AddMessage") && !c.api.ends_with("ShouldAddMessage")).collect();
    let number = |s: &String| s.trim().parse::<i32>().ok();
    let messages = w
        .settings
        .messages
        .iter()
        .zip(posted)
        .enumerate()
        .map(|(i, (line, tick))| {
            // "[to P, sound S] text", as op2-missionhost keeps them.
            let (head, text) = line.strip_prefix('[').and_then(|l| l.split_once("] ")).unwrap_or(("", line));
            let field = |name: &str| head.split(", ").find_map(|f| f.strip_prefix(name)?.trim().parse().ok());
            let call = calls.get(i);
            // (x, y, text, ...) in pixels, or (Unit, text, ...): the unit's tile. Either becomes a file tile (game x
            // less the pad, 32 pixels a tile); a negative pixel y gives no location.
            let at = call.and_then(|c| {
                let first = c.args.first()?;
                match first.strip_prefix("Unit#") {
                    Some(id) => {
                        let id: u32 = id.parse().ok()?;
                        w.units.iter().find(|u| u.id == id).map(|u| (u.x - w.pad, u.y))
                    }
                    None => Some((number(first)?, number(c.args.get(1)?)?)).filter(|&(_, y)| y >= 0).map(|(x, y)| (x / 32 - w.pad, y / 32)),
                }
            });
            Message {
                tick,
                text: text.to_string(),
                to: field("to ").unwrap_or(-1),
                sound: field("sound ").unwrap_or(0),
                at,
                from: call.map_or_else(String::new, |c| c.phase.clone()),
            }
        })
        .collect();
    Ok(Messages { reached: play.tick(), horizon, stopped, messages, triggers: made })
}

/// One `TethysGame::AddMessage` call in the DLL.
#[derive(Debug, Clone)]
pub struct Found {
    /// Its text, `None` when the mission makes it while it runs (a buffer, or a helper's argument).
    pub text: Option<String>,
    /// The exported functions whose code reaches it, directly or through functions they call.
    pub functions: BTreeSet<String>,
}

/// The mission's `TethysGame::AddMessage` calls, read from the DLL without running it, in the order they appear:
/// each call's text (the relocated `push offset text` in the bytes before it) and the exported functions reaching it.
pub fn texts(dll: &Path) -> Result<Vec<Found>> {
    let data = std::fs::read(dll)?;
    let pe = PeFile::parse(&data).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let base = pe.image_base;
    // An RVA's file offset, if a section holds it on disk.
    let offset = |rva: u32| pe.sections.iter().find(|s| s.contains(rva) && rva - s.rva < s.raw_size).map(|s| (rva - s.rva + s.raw_offset) as usize);
    // The little-endian dword at a file offset.
    let dword = |at: usize| data.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    // The IAT slots (virtual addresses) of `TethysGame::AddMessage`'s overloads.
    let slots: Vec<u32> = pe
        .imports
        .iter()
        .filter(|i| matches!(&i.name, ImportName::Name(n) if n.contains("AddMessage@TethysGame")))
        .map(|i| base + i.iat_rva)
        .collect();
    if slots.is_empty() {
        return Ok(Vec::new());
    }
    // The RVAs the relocation table patches: a dword there is an address in the image.
    let relocs: BTreeSet<u32> = pe.relocations.iter().copied().collect();
    // The executable sections (IMAGE_SCN_MEM_EXECUTE).
    let code: Vec<_> = pe.sections.iter().filter(|s| s.characteristics & 0x2000_0000 != 0).collect();
    // `call [slot]`, `jmp [slot]` (an import thunk), `call thunk`, and `mov reg, [slot]` followed by `call reg` (MSVC
    // keeps an import it calls repeatedly in a register): up to the next load of that register, at most 2000 bytes on.
    // The bytes are scanned at every offset, not decoded in order, so a match can sit inside another instruction.
    let mut calls = Vec::new();
    let mut thunks = Vec::new();
    for s in &code {
        // Stop 6 bytes short of the section's end: the longest form matched, FF 15 disp32, is 6 bytes.
        let end = s.rva + s.raw_size.min(s.virtual_size).saturating_sub(6);
        for rva in s.rva..end {
            let Some(at) = offset(rva) else { continue };
            let (op, modrm) = (data[at], data[at + 1]);
            // FF 15: `call [disp32]`; FF 25: `jmp [disp32]`.
            if op == 0xFF && matches!(modrm, 0x15 | 0x25) && dword(at + 2).is_some_and(|v| slots.contains(&v)) {
                if modrm == 0x15 { calls.push(rva) } else { thunks.push(rva) }
            }
            // 8B with mod 00, r/m 101: `mov reg, [disp32]`, the register in bits 3-5.
            if op == 0x8B && modrm & 0xC7 == 0x05 && dword(at + 2).is_some_and(|v| slots.contains(&v)) {
                let reg = (modrm >> 3) & 7;
                for r in rva + 6..(rva + 2000).min(end) {
                    let Some(b) = offset(r) else { break };
                    // Any `mov reg, ...`: the register holds something else from here.
                    if data[b] == 0x8B && (data[b + 1] >> 3) & 7 == reg {
                        break;
                    }
                    // FF D0+reg: `call reg`.
                    if data[b] == 0xFF && data[b + 1] == 0xD0 | reg {
                        calls.push(r);
                    }
                }
            }
        }
    }
    calls.dedup();
    // Calls into a thunk: E8 rel32, `call` relative to the next instruction (5 bytes on).
    if !thunks.is_empty() {
        for s in &code {
            for rva in s.rva..s.rva + s.raw_size.min(s.virtual_size).saturating_sub(5) {
                let Some(at) = offset(rva) else { continue };
                if data[at] == 0xE8 && dword(at + 1).is_some_and(|d| thunks.contains(&(rva + 5).wrapping_add(d))) {
                    calls.push(rva);
                }
            }
        }
    }
    calls.sort_unstable();
    let sites: BTreeSet<u32> = calls.iter().copied().collect();
    let mut reached = callers(&pe, &data, &sites, &relocs);
    Ok(calls
        .into_iter()
        .map(|call| {
            // The nearest relocated `push offset` (or `mov reg, offset`, the unit form's) before the call that points
            // at a string: its text argument. Up to 64 bytes back; 68 is `push imm32`, B8-BF `mov reg, imm32`, and a
            // relocation on the immediate marks it as an address. The text is up to 400 bytes, all printable.
            let text = (call.saturating_sub(64)..call).rev().find_map(|p| {
                let at = offset(p)?;
                if !matches!(data[at], 0x68 | 0xB8..=0xBF) || !relocs.contains(&(p + 1)) {
                    return None;
                }
                let target = offset(dword(at + 1)?.checked_sub(base)?)?;
                let bytes: Vec<u8> = data[target..].iter().copied().take_while(|&b| b != 0).take(400).collect();
                let printable = bytes.len() >= 2 && bytes.iter().all(|&b| b >= 0x20);
                printable.then(|| bytes.iter().map(|&b| b as char).collect::<String>())
            });
            // A `sprintf` format ("%s%i%s") pushed for the buffer the call then posts: a text made while it runs.
            let format = |t: &String| t.as_bytes().windows(2).any(|w| w[0] == b'%' && b"sdiuxc".contains(&w[1]));
            let text = text.filter(|t| !format(t));
            Found { text, functions: reached.remove(&call).unwrap_or_default() }
        })
        .collect())
}

/// For each of `sites` (RVAs), the exported functions whose code reaches it. Each function is followed instruction
/// by instruction from its start, taking both sides of a branch and every case of a switch (a relocated jump table);
/// the functions it calls are followed the same way.
fn callers(pe: &PeFile, data: &[u8], sites: &BTreeSet<u32>, relocs: &BTreeSet<u32>) -> BTreeMap<u32, BTreeSet<String>> {
    let base = pe.image_base;
    // An RVA's file offset, if an executable section holds it on disk.
    let code = |rva: u32| {
        pe.sections
            .iter()
            .find(|s| s.characteristics & 0x2000_0000 != 0 && s.contains(rva) && rva - s.rva < s.raw_size)
            .map(|s| (rva - s.rva + s.raw_offset) as usize)
    };
    // The little-endian dword at an RVA in any section.
    let dword = |rva: u32| {
        let at = pe.sections.iter().find(|s| s.contains(rva) && rva + 4 - s.rva <= s.raw_size).map(|s| (rva - s.rva + s.raw_offset) as usize)?;
        data.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    // Each function's own sites and the functions it calls, by start RVA.
    let mut functions: BTreeMap<u32, (BTreeSet<u32>, BTreeSet<u32>)> = BTreeMap::new();
    let exports: Vec<(&str, u32)> =
        pe.exports.iter().filter(|e| e.forward.is_none() && code(e.rva).is_some()).filter_map(|e| Some((e.name.as_deref()?, e.rva))).collect();
    // First pass: walk each function once, from the exports and then every function they call.
    let mut todo: Vec<u32> = exports.iter().map(|e| e.1).collect();
    while let Some(start) = todo.pop() {
        if functions.contains_key(&start) {
            continue;
        }
        let (mut own, mut calls) = (BTreeSet::new(), BTreeSet::new());
        let mut seen = BTreeSet::new();
        // Where paths still to follow start; a path ends at an instruction already seen, a return or an undecodable
        // byte. A function stops being walked after 200,000 instructions.
        let mut paths = vec![start];
        while let Some(mut rva) = paths.pop() {
            while seen.len() < 200_000 && seen.insert(rva) {
                let Some(at) = code(rva) else { break };
                let bytes = &data[at..(at + 15).min(data.len())];
                let ins = Decoder::with_ip(32, bytes, u64::from(base + rva), DecoderOptions::NONE).decode();
                if ins.is_invalid() {
                    break;
                }
                if sites.contains(&rva) {
                    own.insert(rva);
                }
                let next = rva + ins.len() as u32;
                let target = (ins.near_branch_target() as u32).wrapping_sub(base);
                match ins.flow_control() {
                    FlowControl::Next | FlowControl::IndirectCall => rva = next,
                    FlowControl::Call => {
                        calls.insert(target);
                        rva = next;
                    }
                    FlowControl::ConditionalBranch => {
                        paths.push(target);
                        rva = next;
                    }
                    FlowControl::UnconditionalBranch if ins.op0_kind() == OpKind::NearBranch32 => rva = target,
                    // `jmp [table + reg*4]`: the table's relocated entries, while they point at code (at most 512).
                    FlowControl::IndirectBranch if ins.memory_base() == iced_x86::Register::None && ins.memory_index_scale() == 4 => {
                        let table = ins.memory_displacement32().wrapping_sub(base);
                        for i in 0..512 {
                            let entry = table + 4 * i;
                            match dword(entry).map(|v| v.wrapping_sub(base)) {
                                Some(case) if relocs.contains(&entry) && code(case).is_some() => paths.push(case),
                                _ => break,
                            }
                        }
                        break;
                    }
                    // A return, an indirect jump of another form, or anything else ends the path.
                    _ => break,
                }
            }
        }
        todo.extend(calls.iter().copied());
        functions.insert(start, (own, calls));
    }
    // Second pass: each export's sites are its own and those of every function it reaches.
    let mut out: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    for (name, start) in exports {
        let mut stack = vec![start];
        let mut visited = BTreeSet::new();
        while let Some(f) = stack.pop() {
            if !visited.insert(f) {
                continue;
            }
            if let Some((own, calls)) = functions.get(&f) {
                for site in own {
                    out.entry(*site).or_default().insert(name.to_string());
                }
                stack.extend(calls.iter().copied());
            }
        }
    }
    out
}

/// What the messages window shows: the DLL's message texts, each with who posts it and its first appearance in the
/// replay, and the replay's messages in order.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// The distinct texts found in the DLL, in the order of their first call.
    pub texts: Vec<Text>,
    /// How many `AddMessage` calls have a text the mission makes while it runs.
    pub runtime: usize,
    /// The functions those calls are in, and what runs them.
    pub runtime_from: Vec<Poster>,
    pub replay: Messages,
}

/// One message text from the DLL, with the calls and functions that post it.
#[derive(Debug, Clone, Serialize)]
pub struct Text {
    pub text: String,
    /// How many `AddMessage` calls post it.
    pub calls: usize,
    /// The functions that post it, and what calls them.
    pub from: Vec<Poster>,
    /// Its first message in the replay (an index into `replay.messages`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<usize>,
}

/// A mission function that posts a message, and what runs it.
#[derive(Debug, Clone, Serialize)]
pub struct Poster {
    pub function: String,
    /// The setup, the game's every-tick `AIProc`, or the triggers that call it ("#3 TimeTrigger: every 460 marks").
    /// Empty when nothing in the setup or the replay calls it: a trigger made after the replay stopped, or one
    /// waiting on the player.
    pub by: Vec<String>,
}

impl Poster {
    /// `function` with what calls it, from the triggers the setup and the replay made.
    fn new(function: &str, made: &[Made]) -> Poster {
        let by = match function {
            "InitProc" => vec!["the setup".to_string()],
            "AIProc" => vec!["the game, every tick".to_string()],
            f => made
                .iter()
                .filter(|m| m.trigger.callback.as_deref() == Some(f))
                .map(|m| {
                    let t = &m.trigger;
                    let later = if m.tick > 0 { format!(", made at mark {}", m.tick / MARK) } else { String::new() };
                    format!("#{} {}: {}{later}", t.id, t.kind, t.what)
                })
                .collect(),
        };
        Poster { function: function.to_string(), by }
    }
}

/// Reads the DLL's message texts and replays the mission for `marks` marks.
pub fn report(dll: &Path, start: StartOptions, marks: u32) -> Result<Report> {
    let found = texts(dll)?;
    let replay = replay(dll, start, marks)?;
    let posters = |functions: &BTreeSet<String>| functions.iter().map(|f| Poster::new(f, &replay.triggers)).collect::<Vec<_>>();
    // Each distinct text, its number of calls and the functions they're in; calls with no text are counted apart.
    let mut texts: Vec<(String, usize, BTreeSet<String>)> = Vec::new();
    let mut runtime = 0;
    let mut runtime_from = BTreeSet::new();
    for f in found {
        match f.text {
            Some(t) => match texts.iter_mut().find(|x| x.0 == t) {
                Some(x) => {
                    x.1 += 1;
                    x.2.extend(f.functions);
                }
                None => texts.push((t, 1, f.functions)),
            },
            None => {
                runtime += 1;
                runtime_from.extend(f.functions);
            }
        }
    }
    let texts = texts
        .into_iter()
        .map(|(text, calls, functions)| {
            let first = replay.messages.iter().position(|m| m.text == text);
            Text { from: posters(&functions), text, calls, first }
        })
        .collect();
    Ok(Report { texts, runtime, runtime_from: posters(&runtime_from), replay })
}
