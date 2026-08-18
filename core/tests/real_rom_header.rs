//! Integration test: loads the real development ROM from `roms/` and
//! validates that the header parser reads it correctly end to end.
//!
//! The ROM is a user-provided, gitignored fixture (see `roms/.gitkeep`).
//! If it is missing, this test fails with a clear message instead of
//! silently skipping, so CI/dev setups notice the fixture is absent.

use std::path::PathBuf;

use gba_core::cartridge::{GbaHeader, HEADER_SIZE};

fn rom_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("roms")
        .join("Pokemon - FireRed Version (USA, Europe).gba")
}

#[test]
fn loads_and_parses_real_rom_header() {
    let path = rom_path();
    let rom = std::fs::read(&path).unwrap_or_else(|err| {
        panic!(
            "expected development ROM at {}: {err}\n\
             This fixture is user-provided and gitignored — place a GBA ROM there to run this test.",
            path.display()
        )
    });

    assert!(
        rom.len() >= HEADER_SIZE,
        "ROM is smaller than a GBA header ({} bytes)",
        rom.len()
    );

    let header = GbaHeader::parse(&rom).expect("header parsing should succeed for a real ROM");

    assert_eq!(header.game_title, "POKEMON FIRE");
    assert_eq!(header.game_code, "BPRE");
    assert_eq!(header.maker_code, "01");
    assert!(header.fixed_value_valid(), "fixed value byte should be 0x96");
    assert!(header.checksum_valid(), "header checksum should be valid on a real cartridge dump");
}
