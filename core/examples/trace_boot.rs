//! Verified ARM/Thumb execution tracer for debugging real-ROM boot hangs.
//!
//! Born out of debugging why Pokemon FireRed hangs at boot under the
//! bundled Cult-of-GBA/BIOS replacement (see `assets/bios/`): a from-
//! scratch hand-rolled disassembler kept producing subtly wrong mnemonics
//! that led to false leads, so this uses Capstone — a real, independently
//! verified ARM/Thumb disassembler — instead. Kept as a general-purpose
//! tool since the next real-ROM compatibility puzzle will need the same
//! thing.
//!
//! Usage:
//! ```text
//! cargo run --release --example trace_boot -- <rom.gba> [options]
//!
//! Options:
//!   --bios <path>       Real BIOS dump to boot with (defaults to the
//!                        core's own partial HLE if omitted)
//!   --frames <n>        How many video frames to run (default 300)
//!   --watch <hex>       Report every store instruction whose *live*
//!                        effective address matches this address exactly
//!                        (repeatable; matches on the containing halfword,
//!                        so e.g. --watch 0x4000004 also catches a byte
//!                        store to 0x4000005). Cheap enough to run for the
//!                        whole session.
//!   --from-pc <hex>     Once PC first reaches this address, start a full
//!                        disassembled instruction trace (bounded by
//!                        --window), then stop. Useful for seeing exactly
//!                        how execution arrived at a known hang/loop.
//!   --window <n>        Instructions to print once --from-pc triggers,
//!                        counting backwards from the trigger point
//!                        (default 2000) — i.e. this dumps the *approach*
//!                        to --from-pc, not instructions after it.
//!   --watch-dma          Report every DMAxCNT_H write with the enable bit
//!                        set — channel, source, dest, count, timing mode.
//!                        DMA transfers write memory directly (not via a
//!                        CPU store instruction), so --watch alone can't
//!                        see them; this is the only way to catch a
//!                        register getting programmed via DMA instead of
//!                        a direct STR/STRH.
//! ```
//!
//! Example — did the game ever program DISPSTAT for real, and how did it
//! get to a suspected freeze loop at 0x080008aa:
//! ```text
//! cargo run --release --example trace_boot -- roms/game.gba \
//!   --bios assets/bios/gba_bios.bin --frames 300 \
//!   --watch 0x4000004 --watch 0x4000200 --watch 0x4000208 \
//!   --from-pc 0x080008aa --window 4000
//! ```

use std::collections::VecDeque;
use std::path::PathBuf;

use capstone::prelude::*;
use gba_core::cartridge::Cartridge;
use gba_core::emulator::Emulator;

struct Args {
    rom_path: PathBuf,
    bios_path: Option<PathBuf>,
    frames: u32,
    watch: Vec<u32>,
    from_pc: Option<u32>,
    window: u64,
    watch_dma: bool,
    dump_store_at: Vec<u32>,
}

fn parse_hex(s: &str) -> u32 {
    let s = s.trim_start_matches("0x").trim_start_matches("0X");
    u32::from_str_radix(s, 16).unwrap_or_else(|_| panic!("not a hex address: {s}"))
}

fn parse_args() -> Args {
    let mut args = std::env::args().skip(1);
    let rom_path = PathBuf::from(args.next().expect("usage: trace_boot <rom.gba> [options]"));
    let mut bios_path = None;
    let mut frames = 300u32;
    let mut watch = Vec::new();
    let mut from_pc = None;
    let mut window = 2000u64;
    let mut watch_dma = false;
    let mut dump_store_at = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bios" => bios_path = Some(PathBuf::from(args.next().expect("--bios needs a path"))),
            "--frames" => frames = args.next().expect("--frames needs a number").parse().unwrap(),
            "--watch" => watch.push(parse_hex(&args.next().expect("--watch needs a hex address"))),
            "--from-pc" => from_pc = Some(parse_hex(&args.next().expect("--from-pc needs a hex address"))),
            "--window" => window = args.next().expect("--window needs a number").parse().unwrap(),
            "--watch-dma" => watch_dma = true,
            "--dump-store-at" => {
                dump_store_at.push(parse_hex(&args.next().expect("--dump-store-at needs a hex address")))
            }
            other => panic!("unknown option: {other}"),
        }
    }
    Args { rom_path, bios_path, frames, watch, from_pc, window, watch_dma, dump_store_at }
}

/// If `insn` is a store (str/strb/strh) with a memory operand, the *live*
/// effective address computed from current register values — trustworthy
/// even for PC-relative-looking code, since it uses real runtime state
/// rather than trying to statically resolve literal pools (which is where
/// the hand-rolled predecessor of this tool went wrong).
fn store_target_addr(cs: &Capstone, insn: &capstone::Insn, regs: &gba_core::cpu::Registers) -> Option<u32> {
    let mnemonic = insn.mnemonic()?;
    if !mnemonic.starts_with("str") {
        return None;
    }
    let detail = cs.insn_detail(insn).ok()?;
    let arch_detail = detail.arch_detail();
    let arm_detail = arch_detail.arm()?;
    for op in arm_detail.operands() {
        if let arch::arm::ArmOperandType::Mem(mem) = op.op_type {
            let name = cs.reg_name(mem.base())?;
            let base_val = match name.as_str() {
                "sp" => regs.r(13),
                "lr" => regs.r(14),
                "pc" => regs.pc(),
                other if other.starts_with('r') => other[1..].parse::<usize>().ok().map(|n| regs.r(n))?,
                _ => return None,
            };
            return Some((base_val as i64 + mem.disp() as i64) as u32);
        }
    }
    None
}

fn main() {
    let args = parse_args();
    let rom = std::fs::read(&args.rom_path).expect("read ROM");
    let cart = Cartridge::load(rom).expect("valid cartridge");
    let mut emu = match &args.bios_path {
        Some(p) => {
            let bios = std::fs::read(p).expect("read BIOS");
            Emulator::new_with_bios(cart, &bios)
        }
        None => Emulator::new(cart),
    };

    let cs_thumb = Capstone::new().arm().mode(arch::arm::ArchMode::Thumb).detail(true).build().unwrap();
    let cs_arm = Capstone::new().arm().mode(arch::arm::ArchMode::Arm).detail(true).build().unwrap();

    // Pass 1 (if --from-pc given): find exactly which instruction index
    // first reaches it, cheaply (no disassembly).
    let target_index = args.from_pc.map(|target| {
        let cart2 = Cartridge::load(std::fs::read(&args.rom_path).unwrap()).unwrap();
        let mut probe = match &args.bios_path {
            Some(p) => Emulator::new_with_bios(cart2, &std::fs::read(p).unwrap()),
            None => Emulator::new(cart2),
        };
        let mut idx = 0u64;
        loop {
            if probe.cpu.registers.pc() == target {
                break idx;
            }
            probe.step().expect("must not error while locating --from-pc");
            idx += 1;
            if idx > 2_000_000_000 {
                panic!("never reached --from-pc within budget");
            }
        }
    });
    if let (Some(pc), Some(idx)) = (args.from_pc, target_index) {
        eprintln!("--from-pc {pc:#010x} first reached at instruction index {idx}");
    }
    let window_start = target_index.map(|t| t.saturating_sub(args.window));

    let mut recent_pcs: VecDeque<u32> = VecDeque::new();
    let mut suppressed = 0u64;
    let mut index = 0u64;

    // DMA0..3 register bases (SAD, DAD, CNT_L, CNT_H), standard GBA layout.
    const DMA_BASES: [u32; 4] = [0x0400_00B0, 0x0400_00BC, 0x0400_00C8, 0x0400_00D4];
    let mut last_cnt_h = [0u16; 4];

    'outer: for frame in 0u32..args.frames {
        let mut ran = 0u32;
        while ran < gba_core::emulator::CYCLES_PER_FRAME {
            let pc = emu.cpu.registers.pc();
            let thumb = emu.cpu.registers.in_thumb_state();

            if args.watch_dma {
                for (ch, &base) in DMA_BASES.iter().enumerate() {
                    let cnt_h = emu.bus.read16(base + 0xA);
                    if cnt_h & 0x8000 != 0 && last_cnt_h[ch] & 0x8000 == 0 {
                        let sad = emu.bus.read32(base);
                        let dad = emu.bus.read32(base + 4);
                        let cnt_l = emu.bus.read16(base + 8);
                        let timing = (cnt_h >> 12) & 0x3;
                        let timing_name = ["immediate", "vblank", "hblank", "special"][timing as usize];
                        eprintln!(
                            "[frame {frame}] DMA{ch} ARMED: pc={pc:#010x} src={sad:#010x} dst={dad:#010x} count={cnt_l} timing={timing_name} cnt_h={cnt_h:#06x}"
                        );
                    }
                    last_cnt_h[ch] = cnt_h;
                }
            }

            let (cs, bytes): (&Capstone, Vec<u8>) = if thumb {
                (&cs_thumb, emu.bus.read16(pc & !1).to_le_bytes().to_vec())
            } else {
                (&cs_arm, emu.bus.read32(pc & !3).to_le_bytes().to_vec())
            };
            if let Ok(insns) = cs.disasm_all(&bytes, pc as u64) {
                if let Some(insn) = insns.iter().next() {
                    if args.dump_store_at.contains(&pc) {
                        let addr = store_target_addr(cs, insn, &emu.cpu.registers);
                        let r = |n: usize| emu.cpu.registers.r(n);
                        eprintln!(
                            "[frame {frame}] DUMP pc={pc:#010x} `{} {}` resolved_store_addr={:?} r0={:#x} r1={:#x} r2={:#x} r3={:#x} r4={:#x} r5={:#x} r6={:#x} r7={:#x}",
                            insn.mnemonic().unwrap_or(""),
                            insn.op_str().unwrap_or(""),
                            addr,
                            r(0), r(1), r(2), r(3), r(4), r(5), r(6), r(7)
                        );
                    }
                    if !args.watch.is_empty() {
                        if let Some(addr) = store_target_addr(cs, insn, &emu.cpu.registers) {
                            if args.watch.iter().any(|&w| w & !1 == addr & !1) {
                                eprintln!(
                                    "[frame {frame}] WATCHED STORE addr={addr:#010x}: pc={pc:#010x} `{} {}`",
                                    insn.mnemonic().unwrap_or(""),
                                    insn.op_str().unwrap_or("")
                                );
                            }
                        }
                    }
                    if let Some(start) = window_start {
                        if index >= start {
                            if recent_pcs.contains(&pc) {
                                suppressed += 1;
                            } else {
                                if suppressed > 0 {
                                    println!("  ... ({suppressed} instructions suppressed, tight loop) ...");
                                    suppressed = 0;
                                }
                                println!(
                                    "{index:9} pc={pc:#010x} {} {}",
                                    insn.mnemonic().unwrap_or("?"),
                                    insn.op_str().unwrap_or("")
                                );
                            }
                            recent_pcs.push_back(pc);
                            if recent_pcs.len() > 24 {
                                recent_pcs.pop_front();
                            }
                        }
                    }
                }
            }
            match emu.step() {
                Ok(cycles) => ran += cycles,
                Err(e) => {
                    eprintln!("stopped: CPU error after {index} instructions: {e:?}");
                    break 'outer;
                }
            }
            index += 1;
            if let Some(t) = target_index {
                if index > t + 5 {
                    break 'outer;
                }
            }
        }
    }
}
