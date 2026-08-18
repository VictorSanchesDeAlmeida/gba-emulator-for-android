mod affine;
mod background;
mod bitmap;
mod blend;
mod color;
mod sprite;
mod window;

pub use color::bgr555_to_rgba8888;

pub const SCREEN_WIDTH: usize = 240;
pub const SCREEN_HEIGHT: usize = 160;

const VRAM_SIZE: usize = 0x18000; // 96KB
const OAM_SIZE: usize = 0x400; // 1KB
const PALETTE_SIZE: usize = 0x400; // 1KB — first half BG, second half OBJ

/// Size of the LCD I/O register block this module owns (0x04000000..
/// +IO_SIZE). Everything else in the I/O page (sound, DMA, timers,
/// keypad, interrupts) is still `Bus`'s flat placeholder until its own
/// etapa claims it, the same pattern this etapa follows for VRAM/OAM/
/// Palette/DISPCNT/DISPSTAT/VCOUNT/BGxCNT.
pub(crate) const IO_SIZE: usize = 0x58;

const HDRAW_CYCLES: u32 = 960; // 240 dots * 4 cycles/dot
const HBLANK_CYCLES: u32 = 272; // 68 dots * 4 cycles/dot
const CYCLES_PER_SCANLINE: u32 = HDRAW_CYCLES + HBLANK_CYCLES; // 1232
const VISIBLE_SCANLINES: u16 = 160;
const TOTAL_SCANLINES: u16 = 228;

/// Which PPU-driven conditions newly occurred during a [`Ppu::tick`] call.
/// `vblank`/`hblank`/`vcounter` are filtered by DISPSTAT's own per-source
/// IRQ-enable bits — what decides whether the CPU actually gets
/// interrupted. `vblank_timing`/`hblank_timing`/`vcounter_raw` are the
/// *unconditional* hardware signal, unaffected by DISPSTAT.
/// `vblank_timing`/`hblank_timing` exist because DMA channels armed for
/// VBlank/HBlank start timing trigger on real scanline transitions
/// regardless of whether any interrupt is enabled — DMA and the CPU
/// interrupt controller are separate hardware units that both happen to
/// watch the same PPU timing, not one gating the other. `vcounter_raw`
/// exists for [`crate::emulator::Emulator`]'s BIOS-compatibility fallback —
/// see its doc comment on `tick_peripherals`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IrqEvents {
    pub vblank: bool,
    pub hblank: bool,
    pub vcounter: bool,
    pub vblank_timing: bool,
    pub hblank_timing: bool,
    pub vcounter_raw: bool,
}

/// Sign-extends a 28-bit value (BGxX/BGxY's actual range) held in a u32.
fn sign_extend_28(value: u32) -> i32 {
    ((value << 4) as i32) >> 4
}

/// Decodes a WINxH/WINxV register pair into (x1, x2, y1, y2). Real
/// hardware treats an inverted or off-screen right/bottom edge as "extend
/// to the screen border" rather than as an empty or wrapping window — see
/// GBATEK's "Garbage Values" note on the window registers.
fn window_rect(h: u16, v: u16) -> (usize, usize, usize, usize) {
    let x1 = (h >> 8) as usize;
    let mut x2 = (h & 0xFF) as usize;
    if x1 > x2 || x2 > SCREEN_WIDTH {
        x2 = SCREEN_WIDTH;
    }
    let y1 = (v >> 8) as usize;
    let mut y2 = (v & 0xFF) as usize;
    if y1 > y2 || y2 > SCREEN_HEIGHT {
        y2 = SCREEN_HEIGHT;
    }
    (x1, x2, y1, y2)
}

/// The GBA's PPU: owns VRAM, OAM and Palette RAM (they're physically part
/// of the PPU on real hardware, which is why they live here rather than in
/// `Bus`), the LCD I/O register block, the scanline/dot timing state
/// machine, and the output framebuffer.
///
/// All 6 video modes render for real — Mode 0 (4 regular/tiled
/// backgrounds), Mode 1 (2 regular + 1 affine), Mode 2 (2 affine), and the
/// bitmap Modes 3/4/5 — composited by priority with sprites (OBJ, regular
/// *and* affine, both mapping modes, 4bpp/8bpp, all shapes/sizes,
/// semi-transparent and OBJ-window modes) on top, windows (WIN0/WIN1/
/// WINOBJ/outside) gating per-layer visibility, mosaic, and the alpha
/// blend / brightness color special effects — down to the backdrop color
/// as the final fallback. Not implemented: mosaic on the bitmap modes
/// (3/4/5 have no per-BG mosaic bit meaningfully wired up here).
///
/// VBlank/HBlank/VCounter interrupt requests are emitted by [`Ppu::tick`]'s
/// return value (Etapa 9); this module itself has no interrupt controller
/// reference and never raises an interrupt directly.
#[derive(Debug)]
pub struct Ppu {
    vram: Vec<u8>,
    oam: Vec<u8>,
    palette: Vec<u8>,
    io: Vec<u8>,
    scanline: u16,
    dot_in_line: u32,
    framebuffer: Vec<u16>,
    frame_ready: bool,
}

impl Ppu {
    pub fn new() -> Self {
        Ppu {
            vram: vec![0; VRAM_SIZE],
            oam: vec![0; OAM_SIZE],
            palette: vec![0; PALETTE_SIZE],
            io: vec![0; IO_SIZE],
            scanline: 0,
            dot_in_line: 0,
            framebuffer: vec![0; SCREEN_WIDTH * SCREEN_HEIGHT],
            frame_ready: false,
        }
    }

    // --- Storage accessors used by `Bus` for address decoding/mirroring ---

    pub(crate) fn vram(&self) -> &[u8] {
        &self.vram
    }
    pub(crate) fn vram_mut(&mut self) -> &mut [u8] {
        &mut self.vram
    }
    pub(crate) fn oam(&self) -> &[u8] {
        &self.oam
    }
    pub(crate) fn oam_mut(&mut self) -> &mut [u8] {
        &mut self.oam
    }
    pub(crate) fn palette(&self) -> &[u8] {
        &self.palette
    }
    pub(crate) fn palette_mut(&mut self) -> &mut [u8] {
        &mut self.palette
    }

    /// Reads a byte from the LCD I/O register block (`offset` relative to
    /// 0x04000000). VCOUNT and DISPSTAT's status bits are computed from
    /// PPU state rather than stored, since they're read-only on real
    /// hardware and driven entirely by the scanline/dot counters.
    pub(crate) fn read_register(&self, offset: usize) -> u8 {
        match offset {
            0x04 => (self.io[0x04] & !0x07) | self.dispstat_status_bits(),
            0x06 => (self.scanline & 0xFF) as u8,
            0x07 => (self.scanline >> 8) as u8,
            _ if offset < IO_SIZE => self.io[offset],
            _ => 0,
        }
    }

    pub(crate) fn write_register(&mut self, offset: usize, value: u8) {
        match offset {
            0x04 => self.io[0x04] = value & !0x07, // bits 0-2 are read-only status
            0x06 | 0x07 => {}                      // VCOUNT is read-only
            _ if offset < IO_SIZE => self.io[offset] = value,
            _ => {}
        }
    }

    fn io_u16(&self, offset: usize) -> u16 {
        self.read_register(offset) as u16 | ((self.read_register(offset + 1) as u16) << 8)
    }

    // --- DISPCNT ---

    pub fn dispcnt(&self) -> u16 {
        self.io_u16(0x00)
    }
    pub fn bg_mode(&self) -> u8 {
        (self.dispcnt() & 0x7) as u8
    }
    pub(crate) fn bg_enabled(&self, bg: usize) -> bool {
        self.dispcnt() & (0x100 << bg) != 0
    }
    pub fn obj_enabled(&self) -> bool {
        self.dispcnt() & 0x1000 != 0
    }
    /// OBJ Character VRAM Mapping mode (DISPCNT bit 6): false = 2D
    /// (sprite tiles addressed on a fixed 32-tile-wide grid), true = 1D
    /// (tiles laid out sequentially per sprite row).
    pub(crate) fn obj_mapping_1d(&self) -> bool {
        self.dispcnt() & 0x40 != 0
    }

    // --- BGxCNT / BGxHOFS / BGxVOFS ---

    pub(crate) fn bg_control(&self, bg: usize) -> u16 {
        self.io_u16(0x08 + bg * 2)
    }
    pub(crate) fn bg_priority(&self, bg: usize) -> u8 {
        (self.bg_control(bg) & 0x3) as u8
    }
    pub(crate) fn bg_char_base(&self, bg: usize) -> usize {
        (((self.bg_control(bg) >> 2) & 0x3) as usize) * 0x4000
    }
    pub(crate) fn bg_screen_base(&self, bg: usize) -> usize {
        (((self.bg_control(bg) >> 8) & 0x1F) as usize) * 0x800
    }
    pub(crate) fn bg_is_8bpp(&self, bg: usize) -> bool {
        self.bg_control(bg) & 0x80 != 0
    }
    pub(crate) fn bg_screen_size(&self, bg: usize) -> u8 {
        ((self.bg_control(bg) >> 14) & 0x3) as u8
    }
    pub(crate) fn bg_hofs(&self, bg: usize) -> u16 {
        self.io_u16(0x10 + bg * 4) & 0x1FF
    }
    pub(crate) fn bg_vofs(&self, bg: usize) -> u16 {
        self.io_u16(0x12 + bg * 4) & 0x1FF
    }

    /// BGxCNT bit 6: whether mosaic (coarse-grained pixel snapping) applies
    /// to this background.
    pub(crate) fn bg_mosaic_enabled(&self, bg: usize) -> bool {
        self.bg_control(bg) & 0x40 != 0
    }

    /// "Display Area Overflow" (BGxCNT bit 13) for an affine background:
    /// true wraps texture coordinates outside the map, false leaves them
    /// transparent. Only meaningful for BG2 (Modes 1/2) and BG3 (Mode 2).
    pub(crate) fn bg_affine_wraps(&self, bg: usize) -> bool {
        self.bg_control(bg) & 0x2000 != 0
    }

    /// The 4 fixed-point (8.8) matrix coefficients for an affine
    /// background (BG2 or BG3 only).
    pub(crate) fn bg_affine_params(&self, bg: usize) -> (i16, i16, i16, i16) {
        let base = if bg == 2 { 0x20 } else { 0x30 };
        (
            self.io_u16(base) as i16,
            self.io_u16(base + 2) as i16,
            self.io_u16(base + 4) as i16,
            self.io_u16(base + 6) as i16,
        )
    }

    /// The affine reference point (BGxX/BGxY), a 28-bit signed value in
    /// 20.8 fixed point, sign-extended to i32. This backend treats it as
    /// constant for the whole frame (the texture coordinate at screen
    /// pixel (0, current scanline) is computed directly from it plus the
    /// scanline offset) rather than modeling the hardware's internal
    /// per-scanline accumulation — accurate as long as a game doesn't
    /// rewrite BGxX/Y mid-frame via an HBlank IRQ, which is an advanced
    /// technique uncommon outside specific effects.
    pub(crate) fn bg_affine_reference(&self, bg: usize) -> (i32, i32) {
        let base = if bg == 2 { 0x28 } else { 0x38 };
        (sign_extend_28(self.io_u32(base)), sign_extend_28(self.io_u32(base + 4)))
    }

    fn io_u32(&self, offset: usize) -> u32 {
        self.io_u16(offset) as u32 | ((self.io_u16(offset + 2) as u32) << 16)
    }

    /// Sprite tile data's VRAM base: 0x10000 in tile modes (0-2), but
    /// 0x14000 in bitmap modes (3-5), since the bitmap buffer(s) occupy
    /// the lower part of VRAM that OBJ tiles would otherwise start at.
    pub(crate) fn obj_char_base(&self) -> usize {
        if self.bg_mode() >= 3 { 0x1_4000 } else { 0x1_0000 }
    }

    /// DISPCNT bit 4: which of the two buffers Modes 4/5 read/display.
    pub(crate) fn display_frame_select(&self) -> bool {
        self.dispcnt() & 0x10 != 0
    }

    // --- Mosaic (0x4C) ---

    /// BG mosaic block size, both dimensions already `+1`'d so `1` means
    /// "no visible effect" rather than needing every caller to remember
    /// the raw-register-stores-size-minus-1 convention.
    pub(crate) fn mosaic_bg_size(&self) -> (u16, u16) {
        let reg = self.io[0x4C];
        (((reg & 0xF) as u16) + 1, ((reg >> 4) as u16) + 1)
    }

    pub(crate) fn mosaic_obj_size(&self) -> (u16, u16) {
        let reg = self.io[0x4D];
        (((reg & 0xF) as u16) + 1, ((reg >> 4) as u16) + 1)
    }

    // --- Windows (WIN0H/V, WIN1H/V, WININ, WINOUT: 0x40-0x4B) ---

    pub(crate) fn win0_enabled(&self) -> bool {
        self.dispcnt() & 0x2000 != 0
    }
    pub(crate) fn win1_enabled(&self) -> bool {
        self.dispcnt() & 0x4000 != 0
    }
    pub(crate) fn winobj_enabled(&self) -> bool {
        self.dispcnt() & 0x8000 != 0
    }
    /// Whether any window is active at all — when none are, window
    /// masking is bypassed entirely and every layer is always visible,
    /// exactly like before this etapa added windows.
    pub(crate) fn any_window_enabled(&self) -> bool {
        self.win0_enabled() || self.win1_enabled() || self.winobj_enabled()
    }

    pub(crate) fn win0_bounds(&self) -> (usize, usize, usize, usize) {
        window_rect(self.io_u16(0x40), self.io_u16(0x44))
    }
    pub(crate) fn win1_bounds(&self) -> (usize, usize, usize, usize) {
        window_rect(self.io_u16(0x42), self.io_u16(0x46))
    }
    /// Low byte = WIN0's per-layer enable bits, high byte = WIN1's.
    pub(crate) fn winin(&self) -> u16 {
        self.io_u16(0x48)
    }
    /// Low byte = outside-all-windows enable bits, high byte = WINOBJ's.
    pub(crate) fn winout(&self) -> u16 {
        self.io_u16(0x4A)
    }

    // --- Color special effects (BLDCNT/BLDALPHA/BLDY: 0x50-0x55) ---

    pub(crate) fn bldcnt(&self) -> u16 {
        self.io_u16(0x50)
    }
    /// 0 = none, 1 = alpha blend, 2 = brightness increase, 3 = decrease.
    pub(crate) fn blend_mode(&self) -> u8 {
        ((self.bldcnt() >> 6) & 0x3) as u8
    }
    /// `layer` follows BLDCNT's own bit order: 0-3 = BG0-3, 4 = OBJ, 5 = backdrop.
    pub(crate) fn is_first_target(&self, layer: usize) -> bool {
        self.bldcnt() & (1 << layer) != 0
    }
    pub(crate) fn is_second_target(&self, layer: usize) -> bool {
        self.bldcnt() & (1 << (layer + 8)) != 0
    }
    /// (EVA, EVB) blend coefficients, each already clamped to the 0-16
    /// range real hardware treats as 0.0-1.0 in 1/16ths.
    pub(crate) fn blend_alpha(&self) -> (u8, u8) {
        let reg = self.io_u16(0x52);
        (((reg & 0x1F) as u8).min(16), (((reg >> 8) & 0x1F) as u8).min(16))
    }
    pub(crate) fn blend_brightness(&self) -> u8 {
        ((self.io_u16(0x54) & 0x1F) as u8).min(16)
    }

    // --- DISPSTAT status (read-only, PPU-driven) ---

    fn in_vblank(&self) -> bool {
        self.scanline >= VISIBLE_SCANLINES
    }
    fn in_hblank(&self) -> bool {
        self.dot_in_line >= HDRAW_CYCLES
    }
    fn lyc(&self) -> u16 {
        self.io[0x05] as u16
    }
    fn vcounter_match(&self) -> bool {
        self.scanline == self.lyc()
    }
    fn vblank_irq_enabled(&self) -> bool {
        self.io[0x04] & 0x08 != 0
    }
    fn hblank_irq_enabled(&self) -> bool {
        self.io[0x04] & 0x10 != 0
    }
    fn vcounter_irq_enabled(&self) -> bool {
        self.io[0x04] & 0x20 != 0
    }
    fn dispstat_status_bits(&self) -> u8 {
        let mut bits = 0u8;
        if self.in_vblank() {
            bits |= 0x01;
        }
        if self.in_hblank() {
            bits |= 0x02;
        }
        if self.vcounter_match() {
            bits |= 0x04;
        }
        bits
    }

    // --- Palette ---

    pub(crate) fn palette_color(&self, index: usize) -> u16 {
        let off = (index & 0xFF) * 2;
        self.palette[off] as u16 | ((self.palette[off + 1] as u16) << 8)
    }

    /// OBJ palette RAM is the second half of the 1KB palette bank
    /// (0x200..0x400), entirely separate from the BG palette.
    pub(crate) fn obj_palette_color(&self, index: usize) -> u16 {
        let off = 0x200 + (index & 0xFF) * 2;
        self.palette[off] as u16 | ((self.palette[off + 1] as u16) << 8)
    }

    // --- Timing / rendering ---

    pub fn scanline(&self) -> u16 {
        self.scanline
    }

    /// Advances the PPU by `cycles` CPU cycles, rendering each visible
    /// scanline's full 240 pixels the instant it's reached (not
    /// dot-by-dot — accurate enough for a non-mid-scanline-effects
    /// renderer, and vastly simpler).
    ///
    /// Renders at the *start* of a scanline (`dot_in_line == 0`), not the
    /// end: `step` is always capped to never carry dot_in_line past
    /// `CYCLES_PER_SCANLINE`, so every wrap lands it at exactly 0, and the
    /// next loop iteration's start-of-line check catches it — including
    /// scanline 0 itself on the very first call, using whatever registers
    /// are configured at that point rather than stale construction-time
    /// state.
    ///
    /// Returns which PPU-driven conditions newly occurred during this call
    /// — see [`IrqEvents`] for the IRQ-gated vs. unconditional-timing
    /// distinction. Each `step` covers at most one scanline (`step` is always
    /// capped to the remainder of the current line), so a single iteration
    /// can never observe more than one HBlank-start or one scanline
    /// boundary, keeping the edge detection below correct even for a
    /// large `cycles` value spanning many scanlines.
    pub fn tick(&mut self, cycles: u32) -> IrqEvents {
        let mut events = IrqEvents::default();
        let mut remaining = cycles;
        while remaining > 0 {
            if self.dot_in_line == 0 && self.scanline < VISIBLE_SCANLINES {
                self.render_scanline(self.scanline);
            }

            let was_hdraw = self.dot_in_line < HDRAW_CYCLES;
            let step = remaining.min(CYCLES_PER_SCANLINE - self.dot_in_line);
            self.dot_in_line += step;
            remaining -= step;

            if was_hdraw && self.dot_in_line >= HDRAW_CYCLES {
                events.hblank_timing = true;
                if self.hblank_irq_enabled() {
                    events.hblank = true;
                }
            }

            if self.dot_in_line >= CYCLES_PER_SCANLINE {
                self.dot_in_line -= CYCLES_PER_SCANLINE;
                self.scanline = (self.scanline + 1) % TOTAL_SCANLINES;
                if self.scanline == VISIBLE_SCANLINES {
                    self.frame_ready = true;
                    events.vblank_timing = true;
                    if self.vblank_irq_enabled() {
                        events.vblank = true;
                    }
                }
                if self.vcounter_match() {
                    events.vcounter_raw = true;
                    if self.vcounter_irq_enabled() {
                        events.vcounter = true;
                    }
                }
            }
        }
        events
    }

    fn render_scanline(&mut self, line: u16) {
        let backdrop = self.palette_color(0);
        let mode = self.bg_mode();

        // Mode 0: BG0-3 all regular/tiled. Mode 1: BG0-1 regular, BG2
        // affine. Mode 2: BG2-3 both affine. Modes 3-5: BG2 is the bitmap
        // layer. Any BG a mode doesn't use is left at None and never wins
        // compositing.
        let mut bg_lines: [[Option<u16>; SCREEN_WIDTH]; 4] = [[None; SCREEN_WIDTH]; 4];
        let active_bgs: &[usize] = match mode {
            0 => &[0, 1, 2, 3],
            1 => &[0, 1, 2],
            2 => &[2, 3],
            3 | 4 | 5 => &[2],
            _ => &[],
        };
        if matches!(mode, 3 | 4 | 5) {
            let mut bmp = [backdrop; SCREEN_WIDTH];
            bitmap::render_scanline(self, mode, line, &mut bmp);
            bg_lines[2] = bmp.map(Some);
        } else {
            for &bg in active_bgs {
                // Mode 0 never has an affine background — BG2 is affine only
                // starting in mode 1 (BG2 alone) and mode 2 (BG2 and BG3
                // both). Matching this to `mode`, not just `bg == 2`, is
                // what the module doc above already documents; this must
                // stay in sync with it.
                let is_affine = (mode == 1 && bg == 2) || (mode == 2 && (bg == 2 || bg == 3));
                if is_affine {
                    affine::render_scanline(self, bg, line, &mut bg_lines[bg]);
                } else {
                    background::render_scanline(self, bg, line, &mut bg_lines[bg]);
                }
            }
        }

        let mut obj_pixels: [Option<(u16, u8, bool)>; SCREEN_WIDTH] = [None; SCREEN_WIDTH];
        let mut obj_window = [false; SCREEN_WIDTH];
        sprite::render_scanline(self, line, &mut obj_pixels, &mut obj_window);

        let blend_mode = self.blend_mode();
        let (eva, evb) = self.blend_alpha();
        let evy = self.blend_brightness();

        let mut row = [backdrop; SCREEN_WIDTH];
        for x in 0..SCREEN_WIDTH {
            let mask = window::layer_mask(self, x, line as usize, obj_window[x]);

            // Candidates visible at this pixel: (layer_id, color, priority,
            // force_blend), layer_id 0-3 = BG0-3, 4 = OBJ — matching
            // BLDCNT's own bit order so `is_first_target`/`is_second_target`
            // can be indexed directly by it.
            let mut candidates: [(u8, u16, u8, bool); 5] = [(0, 0, 0, false); 5];
            let mut n = 0;
            for &bg in active_bgs {
                if mask & window::BG_BIT[bg] != 0 {
                    if let Some(color) = bg_lines[bg][x] {
                        candidates[n] = (bg as u8, color, self.bg_priority(bg), false);
                        n += 1;
                    }
                }
            }
            if mask & window::OBJ_BIT != 0 {
                if let Some((color, priority, force_blend)) = obj_pixels[x] {
                    candidates[n] = (4, color, priority, force_blend);
                    n += 1;
                }
            }
            // OBJ beats a BG at equal priority; among BGs, lower index
            // wins — matching hardware (see `sprite`/`background` doc
            // comments for the same rule applied within each layer kind).
            candidates[..n].sort_by_key(|&(layer, _, priority, _)| (priority, if layer == 4 { 0 } else { layer + 1 }));

            let backdrop_candidate = (5u8, backdrop, u8::MAX, false);
            let top1 = if n > 0 { candidates[0] } else { backdrop_candidate };
            let top2 = if n > 1 { candidates[1] } else { backdrop_candidate };

            // A semi-transparent sprite forces alpha-blend mode even if
            // BLDCNT selects "None" or brightness — a documented hardware
            // exception, not a simplification.
            let effective_mode = if top1.3 { 1 } else { blend_mode };
            let blend_allowed = mask & window::BLEND_BIT != 0;
            row[x] = match effective_mode {
                1 if blend_allowed
                    && (top1.3 || self.is_first_target(top1.0 as usize))
                    && self.is_second_target(top2.0 as usize) =>
                {
                    blend::alpha_blend(top1.1, top2.1, eva, evb)
                }
                2 if blend_allowed && self.is_first_target(top1.0 as usize) => blend::brightness_increase(top1.1, evy),
                3 if blend_allowed && self.is_first_target(top1.0 as usize) => blend::brightness_decrease(top1.1, evy),
                _ => top1.1,
            };
        }

        let base = line as usize * SCREEN_WIDTH;
        self.framebuffer[base..base + SCREEN_WIDTH].copy_from_slice(&row);
    }

    /// The full 240x160 framebuffer, native GBA 15-bit BGR555 per pixel,
    /// row-major. Independent of any UI framework — see [`bgr555_to_rgba8888`]
    /// for converting individual pixels to display.
    pub fn framebuffer(&self) -> &[u16] {
        &self.framebuffer
    }

    /// True once per frame, right as VBlank begins. Consumers should check
    /// and clear this (`take_frame_ready`) to know when to present a frame.
    pub fn take_frame_ready(&mut self) -> bool {
        std::mem::take(&mut self.frame_ready)
    }
}

impl Default for Ppu {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vcount_is_read_only_and_tracks_the_scanline() {
        let mut ppu = Ppu::new();
        ppu.write_register(0x06, 0xFF); // ignored: VCOUNT is read-only
        assert_eq!(ppu.read_register(0x06), 0);

        ppu.tick(CYCLES_PER_SCANLINE * 5);
        assert_eq!(ppu.scanline(), 5);
        assert_eq!(ppu.read_register(0x06), 5);
    }

    #[test]
    fn dispstat_vblank_flag_tracks_scanline_160() {
        let mut ppu = Ppu::new();
        assert_eq!(ppu.read_register(0x04) & 0x01, 0);

        ppu.tick(CYCLES_PER_SCANLINE * VISIBLE_SCANLINES as u32);
        assert_eq!(ppu.scanline(), VISIBLE_SCANLINES);
        assert_eq!(ppu.read_register(0x04) & 0x01, 0x01, "VBlank flag must be set at scanline 160");
        assert!(ppu.take_frame_ready());
        assert!(!ppu.take_frame_ready(), "frame_ready clears after being taken");
    }

    #[test]
    fn dispstat_hblank_flag_tracks_dot_position() {
        let mut ppu = Ppu::new();
        assert_eq!(ppu.read_register(0x04) & 0x02, 0);
        ppu.tick(HDRAW_CYCLES);
        assert_eq!(ppu.read_register(0x04) & 0x02, 0x02, "HBlank starts right after the 960 HDraw cycles");
    }

    #[test]
    fn dispstat_vcounter_match_uses_lyc() {
        let mut ppu = Ppu::new();
        ppu.write_register(0x05, 10); // LYC = 10
        ppu.tick(CYCLES_PER_SCANLINE * 10);
        assert_eq!(ppu.scanline(), 10);
        assert_eq!(ppu.read_register(0x04) & 0x04, 0x04);
    }

    #[test]
    fn scanline_wraps_after_a_full_frame() {
        let mut ppu = Ppu::new();
        ppu.tick(CYCLES_PER_SCANLINE * TOTAL_SCANLINES as u32);
        assert_eq!(ppu.scanline(), 0);
    }

    #[test]
    fn dispcnt_roundtrips_and_reports_bg_mode_and_enables() {
        let mut ppu = Ppu::new();
        ppu.write_register(0x00, 0x01); // mode 1
        ppu.write_register(0x01, 0x1F); // BG0-3 + OBJ enabled (bits 8-12)
        assert_eq!(ppu.bg_mode(), 1);
        assert!(ppu.bg_enabled(0));
        assert!(ppu.bg_enabled(3));
        assert!(ppu.obj_enabled());
    }

    #[test]
    fn vblank_irq_event_only_fires_when_dispstat_enables_it() {
        let mut ppu = Ppu::new();
        let events = ppu.tick(CYCLES_PER_SCANLINE * VISIBLE_SCANLINES as u32);
        assert!(!events.vblank, "DISPSTAT bit3 was never set");

        let mut ppu = Ppu::new();
        ppu.write_register(0x04, 0x08); // VBlank IRQ enable
        let events = ppu.tick(CYCLES_PER_SCANLINE * VISIBLE_SCANLINES as u32);
        assert!(events.vblank);
    }

    #[test]
    fn vblank_and_hblank_timing_events_fire_unconditionally_for_dma() {
        // DMA channels armed for VBlank/HBlank start timing must trigger
        // on the raw scanline transition even when DISPSTAT's IRQ-enable
        // bits are untouched — real hardware's DMA controller doesn't
        // care whether the CPU interrupt for the same event is enabled.
        let mut ppu = Ppu::new(); // DISPSTAT IRQ-enable bits are off by default
        let events = ppu.tick(HDRAW_CYCLES);
        assert!(events.hblank_timing, "HBlank timing must fire regardless of DISPSTAT");
        assert!(!events.hblank, "but the IRQ-gated event must not, since it's disabled");

        let events = ppu.tick(CYCLES_PER_SCANLINE * (VISIBLE_SCANLINES as u32 - 1) + HBLANK_CYCLES);
        assert!(events.vblank_timing, "VBlank timing must fire regardless of DISPSTAT");
        assert!(!events.vblank, "but the IRQ-gated event must not, since it's disabled");
    }

    #[test]
    fn hblank_irq_event_fires_once_per_scanline_including_during_vblank() {
        let mut ppu = Ppu::new();
        ppu.write_register(0x04, 0x10); // HBlank IRQ enable

        let events = ppu.tick(HDRAW_CYCLES - 1);
        assert!(!events.hblank, "still inside HDraw");
        let events = ppu.tick(1);
        assert!(events.hblank, "crossing the HDRAW_CYCLES boundary must fire it");

        // Advance well into VBlank and confirm HBlank still fires there too.
        ppu.tick(CYCLES_PER_SCANLINE * 200 - HDRAW_CYCLES - 1);
        let events = ppu.tick(HDRAW_CYCLES + 1);
        assert!(events.hblank, "HBlank timing continues through VBlank lines");
    }

    #[test]
    fn vcounter_irq_event_fires_on_lyc_match_when_enabled() {
        let mut ppu = Ppu::new();
        ppu.write_register(0x05, 10); // LYC = 10
        ppu.write_register(0x04, 0x20); // VCounter IRQ enable

        let events = ppu.tick(CYCLES_PER_SCANLINE * 9);
        assert!(!events.vcounter);
        let events = ppu.tick(CYCLES_PER_SCANLINE);
        assert_eq!(ppu.scanline(), 10);
        assert!(events.vcounter);
    }

    #[test]
    fn backdrop_fills_the_frame_when_no_background_is_enabled() {
        let mut ppu = Ppu::new();
        ppu.palette_mut()[0] = 0x1F; // low byte of palette[0]: red=0x1F, rest 0
        ppu.palette_mut()[1] = 0x00;
        ppu.tick(CYCLES_PER_SCANLINE); // render scanline 0
        assert_eq!(ppu.framebuffer()[0], 0x001F);
        assert_eq!(ppu.framebuffer()[SCREEN_WIDTH - 1], 0x001F);
    }
}
