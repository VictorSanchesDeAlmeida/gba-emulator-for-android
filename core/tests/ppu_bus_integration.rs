//! Integration test: drives the PPU entirely through the real `Bus`
//! address space (0x04000000 registers, 0x06000000 VRAM, 0x05000000
//! palette) and the real `Emulator` cycle clock — not the `Ppu`'s own
//! internal test helpers — to prove the whole CPU-cycle -> Bus -> PPU
//! pipeline wired up in this etapa actually produces a frame.

use gba_core::cartridge::{Cartridge, HEADER_SIZE};
use gba_core::emulator::{Emulator, CYCLES_PER_SCANLINE};
use gba_core::ppu::{bgr555_to_rgba8888, SCREEN_HEIGHT, SCREEN_WIDTH};

fn test_emulator() -> Emulator {
    Emulator::new(Cartridge::load(vec![0u8; HEADER_SIZE + 16]).unwrap())
}

#[test]
fn a_tile_written_through_the_bus_appears_in_the_rendered_frame() {
    let mut emu = test_emulator();

    // DISPCNT: mode 0, BG0 enabled (bit 8).
    emu.bus.write16(0x0400_0000, 0x0100);
    // BG0CNT: char base 0, screen base 0, 4bpp, 32x32 (all zero is fine).
    emu.bus.write16(0x0400_0008, 0x0000);

    // Tile map entry (0,0): tile index 1, palette bank 0.
    emu.bus.write16(0x0600_0000, 0x0001);
    // Tile 1 (char base 0 + 1*32 bytes): pixel 0 = color index 5.
    emu.bus.write8(0x0600_0020, 0x05);
    // BG palette color 5 (byte offset 5*2=10 in palette RAM) = pure blue.
    emu.bus.write16(0x0500_000A, 0x7C00);

    emu.run_cycles(CYCLES_PER_SCANLINE).unwrap(); // render scanline 0

    let pixel = emu.bus.ppu().framebuffer()[0];
    assert_eq!(pixel, 0x7C00);
    assert_eq!(bgr555_to_rgba8888(pixel), [0, 0, 255, 255]);
}

#[test]
fn a_sprite_written_through_the_bus_appears_in_the_rendered_frame() {
    let mut emu = test_emulator();

    // DISPCNT: OBJ enabled (bit 12).
    emu.bus.write16(0x0400_0000, 0x1000);

    // OAM entry 0 (0x07000000 + 0): Y=5, 4bpp, shape=0 (square); X=10,
    // size=0 (8x8), no flip; tile 1, priority 0, palette bank 0.
    emu.bus.write16(0x0700_0000, 5); // attr0
    emu.bus.write16(0x0700_0002, 10); // attr1
    emu.bus.write16(0x0700_0004, 1); // attr2

    // OBJ tile 1 (char base 0x06010000 + 1*32 bytes): pixel 0 = color index 7.
    emu.bus.write8(0x0601_0020, 0x07);
    // OBJ palette color 7 (byte offset 0x200 + 7*2 = 0x20E) = pure green.
    emu.bus.write16(0x0500_020E, 0x03E0);

    emu.run_cycles(CYCLES_PER_SCANLINE * 6).unwrap(); // render through scanline 5

    let pixel = emu.bus.ppu().framebuffer()[5 * SCREEN_WIDTH + 10];
    assert_eq!(pixel, 0x03E0);
    assert_eq!(bgr555_to_rgba8888(pixel), [0, 255, 0, 255]);
}

#[test]
fn mode3_bitmap_written_through_the_bus_appears_in_the_rendered_frame() {
    let mut emu = test_emulator();

    // DISPCNT: mode 3.
    emu.bus.write16(0x0400_0000, 0x0003);
    // Pixel (0,0) of the Mode 3 framebuffer: direct color, pure red.
    emu.bus.write16(0x0600_0000, 0x001F);

    emu.run_cycles(CYCLES_PER_SCANLINE).unwrap();

    let pixel = emu.bus.ppu().framebuffer()[0];
    assert_eq!(pixel, 0x001F);
    assert_eq!(bgr555_to_rgba8888(pixel), [255, 0, 0, 255]);
}

#[test]
fn vblank_flag_becomes_visible_to_cpu_code_through_the_bus() {
    let mut emu = test_emulator();
    assert_eq!(emu.bus.read8(0x0400_0004) & 0x01, 0);

    emu.run_cycles(CYCLES_PER_SCANLINE * 160).unwrap();
    assert_eq!(emu.bus.read8(0x0400_0004) & 0x01, 0x01, "CPU-visible VBlank flag");
    assert_eq!(emu.bus.ppu().framebuffer().len(), SCREEN_WIDTH * SCREEN_HEIGHT);
}
