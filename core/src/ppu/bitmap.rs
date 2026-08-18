use super::{Ppu, SCREEN_WIDTH};

const MODE5_WIDTH: usize = 160;
const MODE5_HEIGHT: usize = 128;
const FRAME_1_OFFSET: usize = 0xA000;

/// Renders one scanline of Modes 3, 4 or 5 — the GBA's direct/paletted
/// framebuffer modes, which replace all 4 tiled backgrounds with a single
/// bitmap that BG2's priority controls. `out` must already be pre-filled
/// with the backdrop color: Mode 5's buffer is smaller than the screen, so
/// pixels outside it are left untouched here rather than overwritten.
pub(super) fn render_scanline(ppu: &Ppu, mode: u8, line: u16, out: &mut [u16; SCREEN_WIDTH]) {
    match mode {
        3 => render_mode3(ppu, line, out),
        4 => render_mode4(ppu, line, out),
        5 => render_mode5(ppu, line, out),
        _ => {}
    }
}

/// Mode 3: 240x160, one buffer, 16-bit direct BGR555 color per pixel.
fn render_mode3(ppu: &Ppu, line: u16, out: &mut [u16; SCREEN_WIDTH]) {
    let vram = ppu.vram();
    let row_base = line as usize * SCREEN_WIDTH * 2;
    for (x, pixel) in out.iter_mut().enumerate() {
        let addr = row_base + x * 2;
        *pixel = vram.get(addr).copied().unwrap_or(0) as u16
            | (vram.get(addr + 1).copied().unwrap_or(0) as u16) << 8;
    }
}

/// Mode 4: 240x160, double-buffered, 8-bit index into the BG palette.
fn render_mode4(ppu: &Ppu, line: u16, out: &mut [u16; SCREEN_WIDTH]) {
    let frame_base = if ppu.display_frame_select() { FRAME_1_OFFSET } else { 0 };
    let vram = ppu.vram();
    let row_base = frame_base + line as usize * SCREEN_WIDTH;
    for (x, pixel) in out.iter_mut().enumerate() {
        let index = vram.get(row_base + x).copied().unwrap_or(0) as usize;
        *pixel = ppu.palette_color(index);
    }
}

/// Mode 5: 160x128 (smaller than the screen), double-buffered, 16-bit
/// direct color.
fn render_mode5(ppu: &Ppu, line: u16, out: &mut [u16; SCREEN_WIDTH]) {
    if line as usize >= MODE5_HEIGHT {
        return; // outside the buffer: leave the pre-filled backdrop alone
    }
    let frame_base = if ppu.display_frame_select() { FRAME_1_OFFSET } else { 0 };
    let vram = ppu.vram();
    let row_base = frame_base + line as usize * MODE5_WIDTH * 2;
    for x in 0..MODE5_WIDTH {
        let addr = row_base + x * 2;
        out[x] = vram.get(addr).copied().unwrap_or(0) as u16
            | (vram.get(addr + 1).copied().unwrap_or(0) as u16) << 8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode3_reads_direct_color_per_pixel() {
        let mut ppu = Ppu::new();
        ppu.vram_mut()[0] = 0x00;
        ppu.vram_mut()[1] = 0x7C; // pixel 0 = 0x7C00 (blue)

        let mut out = [0u16; SCREEN_WIDTH];
        render_mode3(&ppu, 0, &mut out);
        assert_eq!(out[0], 0x7C00);
    }

    #[test]
    fn mode4_indexes_into_the_bg_palette_and_respects_frame_select() {
        let mut ppu = Ppu::new();
        ppu.vram_mut()[0] = 7; // buffer 0, pixel 0 -> palette index 7
        ppu.vram_mut()[0xA000] = 9; // buffer 1, pixel 0 -> palette index 9
        ppu.palette_mut()[7 * 2] = 0x11;
        ppu.palette_mut()[9 * 2] = 0x22;

        let mut out = [0u16; SCREEN_WIDTH];
        render_mode4(&ppu, 0, &mut out);
        assert_eq!(out[0], 0x0011, "frame select defaults to buffer 0");

        ppu.write_register(0x00, 0x10); // DISPCNT: frame select bit (bit4)
        render_mode4(&ppu, 0, &mut out);
        assert_eq!(out[0], 0x0022, "frame select switches to buffer 1");
    }

    #[test]
    fn mode5_is_smaller_than_the_screen_and_leaves_the_rest_at_backdrop() {
        let mut ppu = Ppu::new();
        ppu.vram_mut()[0] = 0xFF;
        ppu.vram_mut()[1] = 0x7F; // pixel 0 = white

        let backdrop = 0x1234;
        let mut out = [backdrop; SCREEN_WIDTH];
        render_mode5(&ppu, 0, &mut out);
        assert_eq!(out[0], 0x7FFF);
        assert_eq!(out[MODE5_WIDTH], backdrop, "past the 160px-wide buffer");

        let mut out_below = [backdrop; SCREEN_WIDTH];
        render_mode5(&ppu, MODE5_HEIGHT as u16, &mut out_below);
        assert_eq!(out_below[0], backdrop, "past the 128-line-tall buffer");
    }
}
