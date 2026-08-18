use super::{Ppu, SCREEN_WIDTH};

/// Renders one affine (rotated/scaled) background's scanline — BG2 in
/// Mode 1, BG2 or BG3 in Mode 2. Unlike regular backgrounds, affine BGs:
/// - use a single 8bpp (256-color) palette, never 4bpp banks;
/// - have 1-byte tile map entries (just a tile index, no flip/palette bits);
/// - are square (128x128 up to 1024x1024 px) with their own screen-size
///   encoding;
/// - either clamp to transparent or wrap at the map edge, per BGxCNT bit 13.
///
/// See [`Ppu::bg_affine_reference`] for the frame-constant simplification
/// this makes for the reference point.
pub(super) fn render_scanline(ppu: &Ppu, bg: usize, line: u16, out: &mut [Option<u16>; SCREEN_WIDTH]) {
    if !ppu.bg_enabled(bg) {
        *out = [None; SCREEN_WIDTH];
        return;
    }

    let (pa, pb, pc, pd) = ppu.bg_affine_params(bg);
    let (ref_x, ref_y) = ppu.bg_affine_reference(bg);
    let size_px = affine_size_px(ppu.bg_screen_size(bg)) as i32;
    let wraps = ppu.bg_affine_wraps(bg);
    let char_base = ppu.bg_char_base(bg);
    let screen_base = ppu.bg_screen_base(bg);
    let map_tiles = (size_px / 8) as usize;
    let vram = ppu.vram();
    let (mosaic_h, mosaic_v) = if ppu.bg_mosaic_enabled(bg) { ppu.mosaic_bg_size() } else { (1, 1) };
    let sy = (line - (line % mosaic_v)) as i32;

    for (x, pixel) in out.iter_mut().enumerate() {
        let sx = (x - (x % mosaic_h as usize)) as i32;
        let tex_x = (ref_x + sx * pa as i32 + sy * pb as i32) >> 8;
        let tex_y = (ref_y + sx * pc as i32 + sy * pd as i32) >> 8;

        let (px, py) = if wraps {
            (tex_x.rem_euclid(size_px), tex_y.rem_euclid(size_px))
        } else if (0..size_px).contains(&tex_x) && (0..size_px).contains(&tex_y) {
            (tex_x, tex_y)
        } else {
            *pixel = None;
            continue;
        };

        let tile_col = (px as usize) / 8;
        let tile_row = (py as usize) / 8;
        let pixel_col = (px as usize) % 8;
        let pixel_row = (py as usize) % 8;

        let map_addr = screen_base + tile_row * map_tiles + tile_col;
        let tile_index = vram.get(map_addr).copied().unwrap_or(0) as usize;

        let tile_addr = char_base + tile_index * 64; // affine BGs are always 8bpp
        let color_index = vram
            .get(tile_addr + pixel_row * 8 + pixel_col)
            .copied()
            .unwrap_or(0) as usize;

        *pixel = if color_index == 0 { None } else { Some(ppu.palette_color(color_index)) };
    }
}

fn affine_size_px(size: u8) -> usize {
    match size {
        0 => 128,
        1 => 256,
        2 => 512,
        3 => 1024,
        _ => unreachable!("2-bit field"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ppu_with_affine_bg2() -> Ppu {
        let mut ppu = Ppu::new();
        ppu.write_register(0x00, 0x01); // DISPCNT: mode 1 (BG2 affine)
        ppu.write_register(0x01, 0x04); // DISPCNT high byte: BG2 enable (bit 10)
        // BG2CNT: char base 0, screen base 0, size 0 (128x128).
        ppu.write_register(0x0C, 0x00);
        ppu.write_register(0x0D, 0x00);
        // Identity matrix: PA=1.0, PB=0, PC=0, PD=1.0 (all 8.8 fixed point).
        ppu.write_register(0x20, 0x00);
        ppu.write_register(0x21, 0x01); // PA = 0x0100 = 1.0
        ppu.write_register(0x26, 0x00);
        ppu.write_register(0x27, 0x01); // PD = 0x0100 = 1.0
        ppu
    }

    #[test]
    fn identity_matrix_samples_the_map_directly() {
        let mut ppu = ppu_with_affine_bg2();
        // Tile map entry at (2,3) [byte-per-entry, 16 tiles/row for 128px]: tile index 5.
        ppu.vram_mut()[3 * 16 + 2] = 5;
        // Tile 5 (8bpp, 64 bytes), pixel (0,0) = color index 9.
        ppu.vram_mut()[5 * 64] = 9;
        ppu.palette_mut()[9 * 2] = 0x55;
        ppu.palette_mut()[9 * 2 + 1] = 0x02;

        let mut out = [None; SCREEN_WIDTH];
        // Screen (16, 24) with identity transform and zero reference -> texture (16,24) -> tile (2,3).
        render_scanline(&ppu, 2, 24, &mut out);
        assert_eq!(out[16], Some(0x0255));
    }

    #[test]
    fn out_of_bounds_is_transparent_unless_wrap_is_set() {
        let mut ppu = ppu_with_affine_bg2();
        ppu.vram_mut()[0] = 1; // tile map entry (0,0) = tile index 1
        ppu.vram_mut()[64] = 9; // tile 1 (char base 0 + 1*64), pixel (0,0) = color index 9
        ppu.palette_mut()[9 * 2] = 0x55;
        ppu.palette_mut()[9 * 2 + 1] = 0x02;

        let mut out = [None; SCREEN_WIDTH];
        // Screen x=200 is way outside a 128px-wide map with zero reference.
        render_scanline(&ppu, 2, 0, &mut out);
        assert_eq!(out[200], None, "clamped (non-wrapping) affine BG is transparent out of bounds");

        // Enable wrap (BGxCNT bit 13).
        ppu.write_register(0x0C, 0x00);
        ppu.write_register(0x0D, 0x20);
        let mut out2 = [None; SCREEN_WIDTH];
        render_scanline(&ppu, 2, 0, &mut out2);
        // 200 mod 128 = 72 -> tile (72/8=9, 0) -> not tile 0 -> transparent too,
        // but must not panic/None-out purely from the bounds check this time.
        // Use x=128 (mod 128 = 0) to land back on tile (0,0) = the tile we set.
        assert!(out2[128].is_some(), "wrapped coordinate 128 mod 128 = 0 hits tile (0,0)");
    }
}
