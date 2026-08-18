//! Per-pixel window masking (WIN0/WIN1/WINOBJ/outside): decides which
//! layers are visible, and whether color special effects can apply, at a
//! given screen pixel.

use super::Ppu;

/// Bit layout matches WININ/WINOUT's per-window enable byte exactly:
/// bits 0-3 = BG0-3, bit 4 = OBJ, bit 5 = color special effect.
pub(super) const BG_BIT: [u8; 4] = [0x01, 0x02, 0x04, 0x08];
pub(super) const OBJ_BIT: u8 = 0x10;
pub(super) const BLEND_BIT: u8 = 0x20;

/// The layer-enable mask in effect at screen `(x, y)`. Checks WIN0 first,
/// then WIN1, then WINOBJ (`in_obj_window`, computed by the sprite
/// renderer from any "OBJ Window"-mode sprite covering this pixel), else
/// falls back to WINOUT's "outside all windows" byte. When DISPCNT enables
/// no window at all, masking is bypassed entirely and everything is
/// enabled — matching real hardware, and keeping every pre-windows test's
/// output unchanged.
pub(super) fn layer_mask(ppu: &Ppu, x: usize, y: usize, in_obj_window: bool) -> u8 {
    if !ppu.any_window_enabled() {
        return 0x3F;
    }
    if ppu.win0_enabled() {
        let (x1, x2, y1, y2) = ppu.win0_bounds();
        if (x1..x2).contains(&x) && (y1..y2).contains(&y) {
            return (ppu.winin() & 0xFF) as u8;
        }
    }
    if ppu.win1_enabled() {
        let (x1, x2, y1, y2) = ppu.win1_bounds();
        if (x1..x2).contains(&x) && (y1..y2).contains(&y) {
            return (ppu.winin() >> 8) as u8;
        }
    }
    if ppu.winobj_enabled() && in_obj_window {
        return (ppu.winout() >> 8) as u8;
    }
    (ppu.winout() & 0xFF) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ppu_with_win0(x1: u8, x2: u8, y1: u8, y2: u8, winin_lo: u8, winout_lo: u8) -> Ppu {
        let mut ppu = Ppu::new();
        ppu.write_register(0x01, 0x20); // DISPCNT high byte: WIN0 enable (bit13)
        ppu.write_register(0x40, x2);
        ppu.write_register(0x41, x1);
        ppu.write_register(0x44, y2);
        ppu.write_register(0x45, y1);
        ppu.write_register(0x48, winin_lo);
        ppu.write_register(0x4A, winout_lo);
        ppu
    }

    #[test]
    fn no_window_enabled_means_everything_is_visible() {
        let ppu = Ppu::new();
        assert_eq!(layer_mask(&ppu, 5, 5, false), 0x3F);
    }

    #[test]
    fn inside_win0_uses_winin_low_byte() {
        let ppu = ppu_with_win0(10, 20, 10, 20, 0x05, 0x00); // WIN0: BG0+BG2
        assert_eq!(layer_mask(&ppu, 15, 15, false), 0x05);
    }

    #[test]
    fn outside_win0_uses_winout_low_byte() {
        let ppu = ppu_with_win0(10, 20, 10, 20, 0x05, 0x1F); // outside: everything but blend
        assert_eq!(layer_mask(&ppu, 0, 0, false), 0x1F);
    }

    #[test]
    fn obj_window_pixel_uses_winout_high_byte() {
        let mut ppu = ppu_with_win0(10, 20, 10, 20, 0x00, 0x00);
        ppu.write_register(0x01, 0x20 | 0x80); // also enable WINOBJ (bit15)
        ppu.write_register(0x4B, 0x10); // WINOBJ enable bits: OBJ only
        assert_eq!(layer_mask(&ppu, 0, 0, true), 0x10);
    }

    #[test]
    fn garbage_bounds_extend_to_the_screen_edge() {
        let mut ppu = Ppu::new();
        ppu.write_register(0x01, 0x20);
        ppu.write_register(0x40, 5); // X2=5
        ppu.write_register(0x41, 200); // X1=200 > X2 -> garbage, X2 becomes SCREEN_WIDTH
        ppu.write_register(0x44, 255);
        ppu.write_register(0x45, 0);
        ppu.write_register(0x48, 0x01);
        // With the garbage fixup, WIN0 spans the whole line, so a far-right pixel still hits it.
        assert_eq!(layer_mask(&ppu, 239, 5, false), 0x01);
    }
}
