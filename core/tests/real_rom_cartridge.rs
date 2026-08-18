//! Integration test: loads the real development ROM through `Cartridge`
//! end to end (file -> header -> save-type detection -> backend).

use std::path::PathBuf;

use gba_core::cartridge::{Cartridge, FlashSize, SaveType};

fn rom_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("roms")
        .join("Pokemon - FireRed Version (USA, Europe).gba")
}

#[test]
fn loads_real_rom_and_detects_its_flash_save_type() {
    let path = rom_path();
    let rom = std::fs::read(&path).unwrap_or_else(|err| {
        panic!(
            "expected development ROM at {}: {err}\n\
             This fixture is user-provided and gitignored — place a GBA ROM there to run this test.",
            path.display()
        )
    });

    let cart = Cartridge::load(rom).expect("Pokemon FireRed should load as a valid cartridge");

    assert_eq!(cart.header().game_code, "BPRE");
    // Fire Red/Leaf Green ship a Sanyo 128KB Flash chip, signalled by the
    // "FLASH1M_V110" string the linker embeds in the ROM.
    assert_eq!(cart.save_type(), SaveType::Flash(FlashSize::Kb128));

    cart_save_roundtrips(cart);
}

fn cart_save_roundtrips(mut cart: Cartridge) {
    cart.write_save_byte(0, 0x00); // no-op without the AA/55 unlock sequence
    assert_eq!(cart.read_save_byte(0), 0xFF, "flash starts erased");
}
