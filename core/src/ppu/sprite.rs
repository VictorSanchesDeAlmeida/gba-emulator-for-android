use super::{Ppu, SCREEN_WIDTH};

/// Renders every enabled, on-screen sprite's contribution to one scanline.
///
/// `out[x]` is `(color, priority, force_blend)` for the winning sprite
/// pixel at that column — `force_blend` is set for "OBJ Mode = semi-
/// transparent" sprites (OAM attr0 bits 10-11 = 1), which real hardware
/// always treats as alpha-blend eligible regardless of BLDCNT's own OBJ
/// bit, provided a valid 2nd-target layer sits beneath them.
///
/// `obj_window[x]` is set wherever an "OBJ Mode = OBJ Window" sprite
/// (bits 10-11 = 2) has an opaque pixel — those sprites are never drawn
/// themselves, they only mark pixels for [`super::window`]'s WINOBJ mask.
///
/// Overlap between ordinary sprites resolves so that lower `priority`
/// wins, and lower OAM index wins ties — matching real hardware, which
/// scans OAM index 0..127 and only lets a later sprite win a pixel it
/// already touched if that sprite's priority is strictly better.
pub(super) fn render_scanline(
    ppu: &Ppu,
    line: u16,
    out: &mut [Option<(u16, u8, bool)>; SCREEN_WIDTH],
    obj_window: &mut [bool; SCREEN_WIDTH],
) {
    *out = [None; SCREEN_WIDTH];
    *obj_window = [false; SCREEN_WIDTH];
    if !ppu.obj_enabled() {
        return;
    }

    let mapping_1d = ppu.obj_mapping_1d();
    let obj_char_base = ppu.obj_char_base();
    let (mosaic_h, mosaic_v) = ppu.mosaic_obj_size();
    let vram = ppu.vram();
    let oam = ppu.oam();

    for index in 0..128 {
        let base = index * 8;
        let a0 = oam[base] as u16 | (oam[base + 1] as u16) << 8;
        let a1 = oam[base + 2] as u16 | (oam[base + 3] as u16) << 8;
        let a2 = oam[base + 4] as u16 | (oam[base + 5] as u16) << 8;

        let affine = (a0 >> 8) & 1 == 1;
        let attr0_bit9 = (a0 >> 9) & 1 == 1;
        let disable = !affine && attr0_bit9; // only means "disable" in non-affine mode
        let double_size = affine && attr0_bit9;
        let obj_mode = (a0 >> 10) & 0x3; // 0=normal, 1=semi-transparent, 2=OBJ window, 3=prohibited
        let mosaic_enabled = (a0 >> 12) & 1 == 1;
        if disable || obj_mode == 3 {
            continue;
        }

        let is_8bpp = (a0 >> 13) & 1 == 1;
        let shape = (a0 >> 14) & 0x3;
        let size_field = (a1 >> 14) & 0x3;
        let Some((w, h)) = shape_size_to_dimensions(shape, size_field) else {
            continue; // shape=3 is a prohibited/reserved encoding
        };
        let (bbox_w, bbox_h) = if double_size { (w * 2, h * 2) } else { (w, h) };

        let sample_line = if mosaic_enabled { line - (line % mosaic_v) } else { line };
        let y = (a0 & 0xFF) as i32;
        let rel_y = (sample_line as i32 - y).rem_euclid(256);
        if rel_y >= bbox_h as i32 {
            continue;
        }

        let x_raw = (a1 & 0x1FF) as i32;
        let x = (x_raw << 23) >> 23; // sign-extend 9 bits

        let tile_base = (a2 & 0x3FF) as usize;
        let priority = ((a2 >> 10) & 0x3) as u8;
        let palette_bank = ((a2 >> 12) & 0xF) as usize;
        let width_tiles = w as usize / 8;

        if affine {
            let param_index = ((a1 >> 9) & 0x1F) as usize;
            let (pa, pb, pc, pd) = read_affine_params(oam, param_index);
            let half_bbox_w = bbox_w as i32 / 2;
            let half_bbox_h = bbox_h as i32 / 2;
            let half_w = w as i32 / 2;
            let half_h = h as i32 / 2;
            let dy = rel_y - half_bbox_h;

            for screen_dx in 0..bbox_w as i32 {
                let screen_x = x + screen_dx;
                if screen_x < 0 || screen_x >= SCREEN_WIDTH as i32 {
                    continue;
                }
                let sample_screen_x =
                    if mosaic_enabled { screen_x - screen_x.rem_euclid(mosaic_h as i32) } else { screen_x };
                let dx = (sample_screen_x - x) - half_bbox_w;

                let tex_x = ((pa as i32 * dx + pb as i32 * dy) >> 8) + half_w;
                let tex_y = ((pc as i32 * dx + pd as i32 * dy) >> 8) + half_h;
                if tex_x < 0 || tex_x >= w as i32 || tex_y < 0 || tex_y >= h as i32 {
                    continue;
                }

                let color_index = sample_color_index(
                    vram,
                    obj_char_base,
                    mapping_1d,
                    tile_base,
                    width_tiles,
                    tex_y as usize / 8,
                    tex_x as usize / 8,
                    tex_y as usize % 8,
                    tex_x as usize % 8,
                    is_8bpp,
                );
                if color_index == 0 {
                    continue;
                }
                let color = resolve_color(ppu, is_8bpp, palette_bank, color_index);
                commit_pixel(&mut out[screen_x as usize], &mut obj_window[screen_x as usize], obj_mode, color, priority);
            }
        } else {
            let hflip = (a1 >> 12) & 1 == 1;
            let vflip = (a1 >> 13) & 1 == 1;
            let py = (if vflip { h as i32 - 1 - rel_y } else { rel_y }) as usize;
            let tile_row = py / 8;
            let pixel_row_in_tile = py % 8;

            for screen_dx in 0..w as i32 {
                let screen_x = x + screen_dx;
                if screen_x < 0 || screen_x >= SCREEN_WIDTH as i32 {
                    continue;
                }
                let sample_screen_x =
                    if mosaic_enabled { screen_x - screen_x.rem_euclid(mosaic_h as i32) } else { screen_x };
                let sample_dx = (sample_screen_x - x).clamp(0, w as i32 - 1);
                let px = (if hflip { w as i32 - 1 - sample_dx } else { sample_dx }) as usize;
                let tile_col = px / 8;
                let pixel_col_in_tile = px % 8;

                let color_index = sample_color_index(
                    vram,
                    obj_char_base,
                    mapping_1d,
                    tile_base,
                    width_tiles,
                    tile_row,
                    tile_col,
                    pixel_row_in_tile,
                    pixel_col_in_tile,
                    is_8bpp,
                );
                if color_index == 0 {
                    continue;
                }
                let color = resolve_color(ppu, is_8bpp, palette_bank, color_index);
                commit_pixel(&mut out[screen_x as usize], &mut obj_window[screen_x as usize], obj_mode, color, priority);
            }
        }
    }
}

fn resolve_color(ppu: &Ppu, is_8bpp: bool, palette_bank: usize, color_index: usize) -> u16 {
    let palette_index = if is_8bpp { color_index } else { palette_bank * 16 + color_index };
    ppu.obj_palette_color(palette_index)
}

fn commit_pixel(
    out_pixel: &mut Option<(u16, u8, bool)>,
    obj_window_pixel: &mut bool,
    obj_mode: u16,
    color: u16,
    priority: u8,
) {
    if obj_mode == 2 {
        *obj_window_pixel = true;
        return;
    }
    let better = match *out_pixel {
        None => true,
        Some((_, existing_priority, _)) => priority < existing_priority,
    };
    if better {
        *out_pixel = Some((color, priority, obj_mode == 1));
    }
}

#[allow(clippy::too_many_arguments)]
fn sample_color_index(
    vram: &[u8],
    obj_char_base: usize,
    mapping_1d: bool,
    tile_base: usize,
    width_tiles: usize,
    tile_row: usize,
    tile_col: usize,
    pixel_row_in_tile: usize,
    pixel_col_in_tile: usize,
    is_8bpp: bool,
) -> usize {
    let col_step = if is_8bpp { 2 } else { 1 };
    let tile_number = if mapping_1d {
        tile_base + tile_row * width_tiles * col_step + tile_col * col_step
    } else {
        tile_base + tile_row * 32 + tile_col * col_step
    };
    let tile_addr = obj_char_base + tile_number * 32;

    if is_8bpp {
        let addr = tile_addr + pixel_row_in_tile * 8 + pixel_col_in_tile;
        vram.get(addr).copied().unwrap_or(0) as usize
    } else {
        let addr = tile_addr + pixel_row_in_tile * 4 + pixel_col_in_tile / 2;
        match vram.get(addr) {
            Some(&byte) => (if pixel_col_in_tile % 2 == 0 { byte & 0xF } else { byte >> 4 }) as usize,
            None => 0,
        }
    }
}

/// OBJ affine parameter set `index` (0-31): PA/PB/PC/PD are each attr3
/// (bytes 6-7) of 4 consecutive OAM entries starting at `index * 4`, in
/// that order — a hardware layout quirk (these 4 OAM slots' attr0-2 are
/// otherwise unused padding when repurposed this way).
fn read_affine_params(oam: &[u8], index: usize) -> (i16, i16, i16, i16) {
    let attr3 = |entry: usize| -> i16 {
        let base = entry * 8 + 6;
        (oam[base] as u16 | (oam[base + 1] as u16) << 8) as i16
    };
    let first_entry = index * 4;
    (attr3(first_entry), attr3(first_entry + 1), attr3(first_entry + 2), attr3(first_entry + 3))
}

fn shape_size_to_dimensions(shape: u16, size: u16) -> Option<(u16, u16)> {
    match (shape, size) {
        (0, 0) => Some((8, 8)),
        (0, 1) => Some((16, 16)),
        (0, 2) => Some((32, 32)),
        (0, 3) => Some((64, 64)),
        (1, 0) => Some((16, 8)),
        (1, 1) => Some((32, 8)),
        (1, 2) => Some((32, 16)),
        (1, 3) => Some((64, 32)),
        (2, 0) => Some((8, 16)),
        (2, 1) => Some((8, 32)),
        (2, 2) => Some((16, 32)),
        (2, 3) => Some((32, 64)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ppu_with_obj_enabled() -> Ppu {
        let mut ppu = Ppu::new();
        ppu.write_register(0x01, 0x10); // DISPCNT high byte: OBJ enable (bit 12)
        ppu
    }

    fn write_oam_entry(ppu: &mut Ppu, index: usize, a0: u16, a1: u16, a2: u16) {
        let base = index * 8;
        let oam = ppu.oam_mut();
        oam[base] = (a0 & 0xFF) as u8;
        oam[base + 1] = (a0 >> 8) as u8;
        oam[base + 2] = (a1 & 0xFF) as u8;
        oam[base + 3] = (a1 >> 8) as u8;
        oam[base + 4] = (a2 & 0xFF) as u8;
        oam[base + 5] = (a2 >> 8) as u8;
    }

    /// Writes attr3 (bytes 6-7) of OAM entry `index` — where affine
    /// parameter sets actually live, distinct from attr0-2.
    fn write_affine_param(ppu: &mut Ppu, index: usize, value: u16) {
        let base = index * 8 + 6;
        let oam = ppu.oam_mut();
        oam[base] = (value & 0xFF) as u8;
        oam[base + 1] = (value >> 8) as u8;
    }

    fn render(ppu: &Ppu, line: u16) -> ([Option<(u16, u8, bool)>; SCREEN_WIDTH], [bool; SCREEN_WIDTH]) {
        let mut out = [None; SCREEN_WIDTH];
        let mut obj_window = [false; SCREEN_WIDTH];
        render_scanline(ppu, line, &mut out, &mut obj_window);
        (out, obj_window)
    }

    #[test]
    fn renders_an_8x8_sprite_pixel_exact() {
        let mut ppu = ppu_with_obj_enabled();
        // Y=10, not affine, not disabled, 4bpp, shape=0 (square).
        // X=20, size=0 (8x8), no flip.
        // tile 1, priority 0, palette bank 3.
        write_oam_entry(&mut ppu, 0, 10, 20, 1 | (3 << 12));

        let tile1 = 0x1_0000 + 32; // OBJ char base + tile_index(1)*32
        ppu.vram_mut()[tile1] = 0x21; // pixel0=1, pixel1=2

        let pal_index1 = 3 * 16 + 1; // bank 3, color 1
        let off1 = 0x200 + pal_index1 * 2;
        ppu.palette_mut()[off1] = 0x1F; // red

        let (out, _) = render(&ppu, 10);

        assert_eq!(out[20], Some((0x001F, 0, false)));
        assert_eq!(out[22], None, "color index 0 is transparent");
        assert_eq!(out[0], None, "outside the sprite's X range");
    }

    #[test]
    fn y_wraps_for_sprites_positioned_above_the_screen() {
        let mut ppu = ppu_with_obj_enabled();
        write_oam_entry(&mut ppu, 0, 250, 0, 0);
        ppu.vram_mut()[0x1_0000 + 6 * 4] = 0x01;
        ppu.palette_mut()[0x200 + 2] = 0x7F;
        ppu.palette_mut()[0x200 + 3] = 0x7F;

        let (out, _) = render(&ppu, 0);
        assert!(out[0].is_some(), "line 0 should hit row 6 of the wrapped sprite");

        let (out2, _) = render(&ppu, 5);
        assert!(out2[0].is_none(), "line 5 is past the 8-tall sprite's wrapped bounds");
    }

    #[test]
    fn lower_oam_index_wins_on_equal_priority_overlap() {
        let mut ppu = ppu_with_obj_enabled();
        write_oam_entry(&mut ppu, 0, 0, 0, 1); // tile 1, priority 0
        write_oam_entry(&mut ppu, 1, 0, 0, 2); // tile 2, priority 0

        ppu.vram_mut()[0x1_0000 + 32] = 0x01;
        ppu.vram_mut()[0x1_0000 + 64] = 0x02;
        ppu.palette_mut()[0x200 + 2] = 0x11;
        ppu.palette_mut()[0x200 + 4] = 0x22;

        let (out, _) = render(&ppu, 0);
        assert_eq!(out[0], Some((0x0011, 0, false)), "sprite 0 (lower OAM index) wins the tie");
    }

    #[test]
    fn better_priority_wins_even_from_a_higher_oam_index() {
        let mut ppu = ppu_with_obj_enabled();
        write_oam_entry(&mut ppu, 0, 0, 0, 1 | (3 << 10)); // priority 3 (worst)
        write_oam_entry(&mut ppu, 1, 0, 0, 2 | (0 << 10)); // priority 0 (best)

        ppu.vram_mut()[0x1_0000 + 32] = 0x01;
        ppu.vram_mut()[0x1_0000 + 64] = 0x02;
        ppu.palette_mut()[0x200 + 2] = 0x11;
        ppu.palette_mut()[0x200 + 4] = 0x22;

        let (out, _) = render(&ppu, 0);
        assert_eq!(out[0], Some((0x0022, 0, false)), "strictly better priority wins regardless of OAM index");
    }

    #[test]
    fn disabled_non_affine_sprite_is_skipped() {
        let mut ppu = ppu_with_obj_enabled();
        write_oam_entry(&mut ppu, 0, (1 << 9) | 0, 0, 1); // disable bit (non-affine)
        ppu.vram_mut()[0x1_0000 + 32] = 0x01;
        ppu.palette_mut()[0x200 + 2] = 0x11;

        let (out, _) = render(&ppu, 0);
        assert!(out.iter().all(|p| p.is_none()));
    }

    #[test]
    fn affine_identity_matrix_renders_like_a_normal_sprite() {
        let mut ppu = ppu_with_obj_enabled();
        // Affine param set 0: PA=1.0, PD=1.0 (identity), stored in OAM
        // entries 0-3's attr3 fields.
        write_affine_param(&mut ppu, 0, 0x0100); // PA
        write_affine_param(&mut ppu, 1, 0); // PB
        write_affine_param(&mut ppu, 2, 0); // PC
        write_affine_param(&mut ppu, 3, 0x0100); // PD

        // Affine sprite at index 4: Y=10, affine=1, param_index=0, X=20, 8x8, tile 1.
        write_oam_entry(&mut ppu, 4, (1 << 8) | 10, 20, 1);
        ppu.vram_mut()[0x1_0000 + 32] = 0x01;
        ppu.palette_mut()[0x200 + 2] = 0x1F;

        let (out, _) = render(&ppu, 10);
        assert_eq!(out[20], Some((0x001F, 0, false)));
    }

    #[test]
    fn affine_double_size_doubles_the_bounding_box_without_moving_the_texture() {
        let mut ppu = ppu_with_obj_enabled();
        write_affine_param(&mut ppu, 0, 0x0100); // PA
        write_affine_param(&mut ppu, 1, 0);
        write_affine_param(&mut ppu, 2, 0);
        write_affine_param(&mut ppu, 3, 0x0100); // PD

        // affine + double-size (bits 8 and 9), Y=0, X=0, 8x8 -> bbox 16x16.
        write_oam_entry(&mut ppu, 4, (1 << 8) | (1 << 9) | 0, 0, 1);
        ppu.vram_mut()[0x1_0000 + 32] = 0x01; // tile1 pixel(0,0)
        ppu.palette_mut()[0x200 + 2] = 0x1F;

        let (out, _) = render(&ppu, 4); // center of the 16-tall bbox is row 8; texture row 0 sits at bbox row 4
        // bbox center (8,8) in an identity transform maps screen(4,4) -> tex(0,0) since dx=4-8=-4, +half_w(4)=0.
        assert_eq!(out[4], Some((0x001F, 0, false)));
    }

    #[test]
    fn semi_transparent_sprite_sets_the_force_blend_flag() {
        let mut ppu = ppu_with_obj_enabled();
        write_oam_entry(&mut ppu, 0, (1 << 10) | 0, 0, 1); // OBJ mode = semi-transparent
        ppu.vram_mut()[0x1_0000 + 32] = 0x01;
        ppu.palette_mut()[0x200 + 2] = 0x11;

        let (out, _) = render(&ppu, 0);
        assert_eq!(out[0], Some((0x0011, 0, true)));
    }

    #[test]
    fn obj_window_mode_sprite_marks_the_window_mask_but_draws_nothing() {
        let mut ppu = ppu_with_obj_enabled();
        write_oam_entry(&mut ppu, 0, (2 << 10) | 0, 0, 1); // OBJ mode = OBJ window
        ppu.vram_mut()[0x1_0000 + 32] = 0x01;
        ppu.palette_mut()[0x200 + 2] = 0x11;

        let (out, obj_window) = render(&ppu, 0);
        assert_eq!(out[0], None, "OBJ-window sprites are never drawn as visible pixels");
        assert!(obj_window[0], "but they do mark the OBJ window mask");
        assert!(!obj_window[1], "only where the sprite is actually opaque");
    }

    #[test]
    fn obj_mosaic_holds_a_sampled_column_across_the_block() {
        let mut ppu = ppu_with_obj_enabled();
        ppu.write_register(0x4D, 0x03); // OBJ mosaic: H size = 4 (3+1)
        // 8x8 sprite, mosaic enabled (attr0 bit12), tile with distinct pixels 0 and 1.
        write_oam_entry(&mut ppu, 0, (1 << 12) | 0, 0, 1);
        ppu.vram_mut()[0x1_0000 + 32] = 0x21; // pixel0=1, pixel1=2
        ppu.palette_mut()[0x200 + 2] = 0x11; // color 1
        ppu.palette_mut()[0x200 + 4] = 0x22; // color 2

        let (out, _) = render(&ppu, 0);
        // Screen x=0..3 all snap to sampled x=0 (color index 1); x=1,2,3 must
        // match x=0 rather than showing their own unsnapped pixel.
        assert_eq!(out[0], out[1]);
        assert_eq!(out[0], out[3]);
    }
}
