//! Integration test: mounts the real development ROM behind the Memory Bus
//! and reads its header through the actual GBA address space (0x08000000+)
//! instead of a raw byte slice, exercising the ROM wait-state window mapping.

use std::path::PathBuf;

use gba_core::cartridge::Cartridge;
use gba_core::memory::Bus;

fn rom_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("roms")
        .join("Pokemon - FireRed Version (USA, Europe).gba")
}

#[test]
fn reads_real_rom_title_through_the_bus_address_space() {
    let path = rom_path();
    let rom = std::fs::read(&path).unwrap_or_else(|err| {
        panic!(
            "expected development ROM at {}: {err}\n\
             This fixture is user-provided and gitignored — place a GBA ROM there to run this test.",
            path.display()
        )
    });

    let cart = Cartridge::load(rom).expect("valid cartridge");
    let bus = Bus::new(cart);

    // Game title lives at header offset 0xA0, mapped at cartridge base
    // 0x08000000 in the CPU's address space.
    let title: Vec<u8> = (0..12).map(|i| bus.read8(0x0800_00A0 + i)).collect();
    assert_eq!(&title, b"POKEMON FIRE");

    // Same bytes, same mirror, through wait-state window 2 (0x0C000000).
    let mirrored: Vec<u8> = (0..12).map(|i| bus.read8(0x0C00_00A0 + i)).collect();
    assert_eq!(mirrored, title);
}
