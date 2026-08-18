//! Integration test: runs the real development ROM's actual first
//! instruction (the entry-point branch every GBA ROM starts with, jumping
//! over the header) through `Emulator`, proving the CPU+Bus+cycle-clock
//! wiring works against real cartridge content, not just hand-assembled
//! test programs.

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
fn executes_the_real_roms_entry_point_branch() {
    let path = rom_path();
    let rom = std::fs::read(&path).unwrap_or_else(|err| {
        panic!(
            "expected development ROM at {}: {err}\n\
             This fixture is user-provided and gitignored — place a GBA ROM there to run this test.",
            path.display()
        )
    });

    let cart = Cartridge::load(rom).expect("valid cartridge");
    let mut emu = Emulator::new(cart);

    const CART_BASE: u32 = 0x0800_0000;
    emu.cpu.registers.set_pc(CART_BASE);

    // The header's entry_point field (offset 0x00) is a real ARM branch
    // instruction every commercial GBA ROM starts with. Decode its target
    // independently from the raw bytes to cross-check against what the
    // CPU actually does.
    let entry_word = emu.bus.read32(CART_BASE);
    let offset = (((entry_word & 0x00FF_FFFF) << 8) as i32 >> 8) << 2;
    let expected_target = CART_BASE.wrapping_add(8).wrapping_add(offset as u32);

    let cycles = emu.step().expect("the entry-point branch is a plain B, already implemented");

    assert_eq!(emu.cpu.registers.pc(), expected_target);
    assert!(cycles > 0);
    assert_eq!(emu.total_cycles(), cycles as u64);
}
