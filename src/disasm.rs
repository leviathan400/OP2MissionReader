//! `op2dll disasm`: a mission DLL's code as x86 instructions, for reading what an exported function does. Imports
//! are named where the code calls or loads them, exports where it calls or jumps to them, and an immediate address
//! that holds text is shown with the text.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result, bail};
use iced_x86::{Decoder, DecoderOptions, FlowControl, Formatter, Instruction, MasmFormatter, OpKind};
use op2_x86::pe::{ImportName, PeFile};

/// The instructions from `what` (an export's name, or an address, `0x...` with the image base) on: `count` of them,
/// or, with no count, up to the first `ret` that no jump seen so far goes past (an error after 5000 instructions).
pub fn disasm(dll: &Path, what: &str, count: Option<usize>) -> Result<String> {
    let data = std::fs::read(dll)?;
    let pe = PeFile::parse(&data).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let base = pe.image_base;
    let start = match what.strip_prefix("0x").or_else(|| what.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).context("not a hex address")?.checked_sub(base).context("below the image base")?,
        None => pe
            .exports
            .iter()
            .find(|e| e.name.as_deref() == Some(what))
            .map(|e| e.rva)
            .with_context(|| format!("{} exports no {what}", dll.display()))?,
    };
    let section = pe
        .sections
        .iter()
        .find(|s| s.contains(start) && start - s.rva < s.raw_size)
        .context("the address isn't in the file's sections")?;
    // Decode from the file's bytes, no further than the section's data on disk.
    let at = (start - section.rva + section.raw_offset) as usize;
    let end = (section.raw_offset + section.raw_size.min(section.virtual_size)) as usize;
    // Each import's IAT slot, by address: the code calls or loads an import through its slot.
    let imports: BTreeMap<u32, String> = pe
        .imports
        .iter()
        .map(|i| {
            let name = match &i.name {
                ImportName::Name(n) => n.clone(),
                other => format!("{other:?}"),
            };
            (base + i.iat_rva, format!("{}!{name}", i.dll))
        })
        .collect();
    let exports: BTreeMap<u32, String> = pe.exports.iter().filter_map(|e| Some((base + e.rva, e.name.clone()?))).collect();
    // The printable, zero-ended text at an address in the file (3 to 120 characters), if any.
    let text_at = |va: u32| -> Option<String> {
        let rva = va.checked_sub(base)?;
        let s = pe.sections.iter().find(|s| s.contains(rva) && rva - s.rva < s.raw_size)?;
        let off = (rva - s.rva + s.raw_offset) as usize;
        let bytes: Vec<u8> = data[off..].iter().take(120).take_while(|b| **b != 0).copied().collect();
        (bytes.len() >= 3 && bytes.iter().all(|b| (0x20..0x7F).contains(b))).then(|| String::from_utf8_lossy(&bytes).into_owned())
    };
    let mut decoder = Decoder::with_ip(32, &data[at..end], u64::from(base + start), DecoderOptions::NONE);
    let mut fmt = MasmFormatter::new();
    let mut out = String::new();
    let mut ins = Instruction::default();
    // The furthest jump target seen: a `ret` before it isn't the function's end.
    let mut furthest = u64::from(base + start);
    let mut n = 0;
    while decoder.can_decode() {
        decoder.decode_out(&mut ins);
        let mut text = String::new();
        fmt.format(&ins, &mut text);
        // The note after the instruction: an export it branches to, else an import it reads through, else the text
        // at an immediate address.
        let mut note = String::new();
        let mem = ins.memory_displacement32();
        if ins.op_kinds().any(|k| k == OpKind::Memory)
            && let Some(name) = imports.get(&mem) {
                note = name.clone();
            }
        if ins.op_kinds().any(|k| k == OpKind::NearBranch32) {
            let target = ins.near_branch32();
            if let Some(name) = exports.get(&target) {
                note = name.clone();
            }
            if matches!(ins.flow_control(), FlowControl::ConditionalBranch | FlowControl::UnconditionalBranch) {
                furthest = furthest.max(u64::from(target));
            }
        }
        if note.is_empty() && ins.op_kinds().any(|k| k == OpKind::Immediate32)
            && let Some(t) = text_at(ins.immediate32()) {
                note = format!("{t:?}");
            }
        let _ = write!(out, "{:08X}  {text}", ins.ip32());
        if !note.is_empty() {
            let _ = write!(out, "    ; {note}");
        }
        out.push('\n');
        n += 1;
        if count.is_some_and(|c| n >= c) {
            break;
        }
        if count.is_none() && ins.flow_control() == FlowControl::Return && ins.next_ip() > furthest {
            break;
        }
        if count.is_none() && n > 5000 {
            bail!("no end found in 5000 instructions; give a count");
        }
    }
    Ok(out)
}
