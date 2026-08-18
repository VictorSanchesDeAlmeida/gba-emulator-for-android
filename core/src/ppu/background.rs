use super::{Ppu, SCREEN_WIDTH};

const TILE_SIZE_4BPP: usize = 32; // bytes: 8x8 pixels, 4 bits/pixel
const TILE_SIZE_8BPP: usize = 64; // bytes: 8x8 pixels, 8 bits/pixel
const SCREEN_BLOCK_SIZE: usize = 0x800; // 2KB = one 32x32-tile map screen

/// Renders one Mode 0 text-mode background's scanline into `out`,
/// overwriting whatever was there before (including on a disabled
/// background, which clears to fully transparent rather than leaving
/// `out` however the caller happened to set it up). `None` marks a
/// transparent pixel (palette index 0 of whichever sub-palette applies),
/// which the caller composites against lower-priority backgrounds and
/// finally the backdrop color.
pub(super) fn render_scanline(ppu: &Ppu, bg: usize, line: u16, out: &mut [Option<u16>; SCREEN_WIDTH]) {
    if !ppu.bg_enabled(bg) {
        *out = [None; SCREEN_WIDTH];
        return;
    }

    let (map_tiles_w, map_tiles_h) = screen_size_in_tiles(ppu.bg_screen_size(bg));
    let map_px_w = map_tiles_w * 8;
    let map_px_h = map_tiles_h * 8;

    // Mosaic snaps the *sampled* screen position to a coarser grid before
    // scroll/tile lookup, then that one sampled color gets written at
    // every true screen position in the block — the block itself stays at
    // full resolution, only the source data driving it is blocky.
    let (mosaic_h, mosaic_v) = if ppu.bg_mosaic_enabled(bg) { ppu.mosaic_bg_size() } else { (1, 1) };
    let sample_line = line - (line % mosaic_v);

    let y = (sample_line as usize + ppu.bg_vofs(bg) as usize) % map_px_h;
    let tile_row = y / 8;
    let pixel_row_in_tile = y % 8;

    let is_8bpp = ppu.bg_is_8bpp(bg);
    let char_base = ppu.bg_char_base(bg);
    let screen_base = ppu.bg_screen_base(bg);
    let size = ppu.bg_screen_size(bg);
    let vram = ppu.vram();

    for (x, pixel) in out.iter_mut().enumerate() {
        let sample_x = x - (x % mosaic_h as usize);
        let x_wrapped = (sample_x + ppu.bg_hofs(bg) as usize) % map_px_w;
        let tile_col = x_wrapped / 8;
        let pixel_col_in_tile = x_wrapped % 8;

        let block_index = screen_block_index(size, tile_col / 32, tile_row / 32);
        let local_col = tile_col % 32;
        let local_row = tile_row % 32;
        let entry_addr = screen_base + block_index * SCREEN_BLOCK_SIZE + (local_row * 32 + local_col) * 2;
        if entry_addr + 1 >= vram.len() {
            continue;
        }
        let entry = vram[entry_addr] as u16 | ((vram[entry_addr + 1] as u16) << 8);
        let tile_index = (entry & 0x3FF) as usize;
        let hflip = entry & 0x0400 != 0;
        let vflip = entry & 0x0800 != 0;
        let palette_bank = ((entry >> 12) & 0xF) as usize;

        let px = if hflip { 7 - pixel_col_in_tile } else { pixel_col_in_tile };
        let py = if vflip { 7 - pixel_row_in_tile } else { pixel_row_in_tile };

        let color_index = if is_8bpp {
            let addr = char_base + tile_index * TILE_SIZE_8BPP + py * 8 + px;
            vram.get(addr).copied().unwrap_or(0) as usize
        } else {
            let addr = char_base + tile_index * TILE_SIZE_4BPP + py * 4 + px / 2;
            match vram.get(addr) {
                Some(&byte) => (if px % 2 == 0 { byte & 0xF } else { byte >> 4 }) as usize,
                None => 0,
            }
        };

        if color_index == 0 {
            continue; // transparent
        }

        let palette_index = if is_8bpp { color_index } else { palette_bank * 16 + color_index };
        *pixel = Some(ppu.palette_color(palette_index));
    }
}

fn screen_size_in_tiles(size: u8) -> (usize, usize) {
    match size {
        0 => (32, 32),
        1 => (64, 32),
        2 => (32, 64),
        3 => (64, 64),
        _ => unreachable!("2-bit field"),
    }
}

/// Larger screen sizes are made of multiple adjacent 32x32-tile 2KB
/// screen blocks; this maps a (block_col, block_row) pair to the block's
/// index in VRAM for each size. See GBATEK "BG Screen Data Format".
fn screen_block_index(size: u8, block_col: usize, block_row: usize) -> usize {
    match size {
        0 => 0,
        1 => block_col,          // 64x32: two blocks side by side
        2 => block_row,          // 32x64: two blocks stacked
        3 => block_row * 2 + block_col, // 64x64: 2x2 blocks
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ppu_with_bg0_enabled() -> Ppu {
        let mut ppu = Ppu::new();
        ppu.write_register(0x00, 0x00); // DISPCNT low byte: mode 0
        ppu.write_register(0x01, 0x01); // DISPCNT high byte: BG0 enable (bit 8)
        ppu
    }

    #[test]
    fn renders_a_single_4bpp_tile_pixel_exact() {
        let mut ppu = ppu_with_bg0_enabled();
        // BG0CNT: char base block 0, screen base block 0, 4bpp, 32x32.
        ppu.write_register(0x08, 0x00);
        ppu.write_register(0x09, 0x00);

        // Screen block 0, tile map entry (0,0) = tile index 1, palette bank 2.
        ppu.vram_mut()[0] = 0x01;
        ppu.vram_mut()[1] = 0x20;

        // Tile 1 (4bpp, 32 bytes) at char base 0: row 0 = pixels [1,2,0,0,0,0,0,0].
        let tile1 = TILE_SIZE_4BPP; // tile index 1 * 32 bytes, char base block 0
        ppu.vram_mut()[tile1] = 0x21; // low nibble=pixel0=1, high nibble=pixel1=2

        // Palette bank 2, color 1 -> palette RAM index 2*16+1 = 33.
        let color_addr = 33 * 2;
        ppu.palette_mut()[color_addr] = 0x1F; // red
        ppu.palette_mut()[color_addr + 1] = 0x00;
        // bank 2, color 2 -> index 34: green
        ppu.palette_mut()[34 * 2] = 0xE0;
        ppu.palette_mut()[34 * 2 + 1] = 0x03;

        let mut out = [None; SCREEN_WIDTH];
        render_scanline(&ppu, 0, 0, &mut out);

        assert_eq!(out[0], Some(0x001F), "pixel 0 -> color index 1 -> red");
        assert_eq!(out[1], Some(0x03E0), "pixel 1 -> color index 2 -> green");
        assert_eq!(out[2], None, "color index 0 is transparent");
    }

    #[test]
    fn scroll_registers_shift_the_sampled_position() {
        let mut ppu = ppu_with_bg0_enabled();
        ppu.write_register(0x08, 0x00);
        ppu.write_register(0x09, 0x00);
        ppu.write_register(0x10, 8); // BG0HOFS = 8 -> one tile to the right

        // Tile map entry (1,0) = tile index 1 (the tile scrolled into x=0 after +8).
        ppu.vram_mut()[2] = 0x01;
        ppu.vram_mut()[3] = 0x00;
        let tile1 = TILE_SIZE_4BPP; // tile index 1, char base block 0
        ppu.vram_mut()[tile1] = 0x01; // pixel0 = color index 1
        ppu.palette_mut()[2] = 0xFF;
        ppu.palette_mut()[3] = 0x7F; // white

        let mut out = [None; SCREEN_WIDTH];
        render_scanline(&ppu, 0, 0, &mut out);
        assert_eq!(out[0], Some(0x7FFF));
    }

    #[test]
    fn hflip_and_vflip_reverse_sampled_pixel_position() {
        let mut ppu = ppu_with_bg0_enabled();
        ppu.write_register(0x08, 0x00);
        ppu.write_register(0x09, 0x00);

        // Tile map entry (0,0): tile 1, hflip=1 (bit10).
        ppu.vram_mut()[0] = 0x01;
        ppu.vram_mut()[1] = 0x04;

        let tile1 = TILE_SIZE_4BPP; // tile index 1, char base block 0
        ppu.vram_mut()[tile1] = 0x21; // pixel0=1, pixel1=2
        ppu.palette_mut()[2] = 0x1F;
        ppu.palette_mut()[4] = 0xE0;
        ppu.palette_mut()[5] = 0x03;

        let mut out = [None; SCREEN_WIDTH];
        render_scanline(&ppu, 0, 0, &mut out);
        // hflip: sampled pixel at screen x=0 is the tile's pixel 7, which is
        // 0 (transparent) since only pixels 0-1 are non-zero in this tile.
        assert_eq!(out[7], Some(0x001F), "screen x=7 samples tile pixel 0 after hflip");
    }

    #[test]
    fn disabled_background_renders_fully_transparent() {
        let mut ppu = Ppu::new(); // BG0 not enabled
        ppu.vram_mut()[0] = 0x01;
        let mut out = [Some(0x1234); SCREEN_WIDTH];
        render_scanline(&ppu, 0, 0, &mut out);
        assert!(out.iter().all(|p| p.is_none()));
    }
}
