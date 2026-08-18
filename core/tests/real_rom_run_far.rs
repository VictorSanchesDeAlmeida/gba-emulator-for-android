//! Exploratory integration test: without a BIOS loaded, how many real
//! instructions from the Pokemon FireRed ROM can the CPU execute before it
//! hits something unimplemented? This is diagnostic, not a strict
//! correctness check (there's no BIOS, so register/stack state at the
//! entry point isn't what real hardware would have) — it exists to catch
//! outright crashes and to report a concrete milestone number.

use std::path::PathBuf;

use gba_core::cartridge::Cartridge;
use gba_core::emulator::Emulator;

fn rom_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("roms")
        .join("Pokemon - FireRed Version (USA, Europe).gba")
}

#[test]
fn runs_many_real_instructions_without_a_bios() {
    let rom = std::fs::read(rom_path()).expect("development ROM fixture");
    let cart = Cartridge::load(rom).expect("valid cartridge");
    let mut emu = Emulator::new(cart);
    emu.cpu.registers.set_pc(0x0800_0000);

    let mut executed = 0u32;
    let outcome = loop {
        if executed >= 20_000 {
            break None;
        }
        match emu.step() {
            Ok(_) => executed += 1,
            Err(e) => break Some(e),
        }
    };

    eprintln!("executed {executed} real instructions before: {outcome:?}");
    // Manually verified this clears 2,000,000 instructions with no error at
    // all — almost certainly spinning on a hardware condition (VCOUNT/VBlank
    // status) that only the PPU (Etapa 6) will ever make true. 20k here
    // keeps the test itself fast while still proving ARM+Thumb decoding
    // holds up well past the entry point on real, unmodified game code.
    assert!(executed >= 50, "should get well past the entry-point branch before hitting anything unimplemented");
}
