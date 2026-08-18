//! FFI bridge between `gba-core` and the mobile shells.
//!
//! Two bridges live in this one crate because they share the same
//! `Instance` type and lifecycle, just exposed through different ABIs:
//! - `jni_bridge`: JNI exports for the Android Kotlin module (`GbaNative`).
//! - `c_bridge`: a plain C ABI for the iOS Swift module, once that's built
//!   on a Mac (not testable on this Windows machine — see the project's
//!   Etapa 7 notes).
//!
//! Neither bridge contains emulation logic. They only translate between
//! `gba-core`'s Rust API and whatever the host platform can call, and
//! convert the framebuffer to a platform-friendly pixel format
//! (`0xAARRGGBB`) on the way out.

mod c_bridge;
mod jni_bridge;

use gba_core::cartridge::Cartridge;
use gba_core::emulator::Emulator;
use gba_core::ppu::bgr555_to_rgba8888;

/// Real GBA BIOS dumps are exactly 16KB (0x4000 bytes) — used here only to
/// reject an obviously-wrong file before it ever reaches the core, so the
/// host app can show a real error instead of a silently-broken boot.
const BIOS_SIZE: usize = 0x4000;

/// Output rate this bridge resamples `gba_core`'s APU into — chosen because
/// it divides the GBA's fixed CPU clock (16,777,216 Hz) evenly into exactly
/// 512 cycles/sample, so [`Instance::run_frame`] can walk cycle-budget
/// chunks without any fractional drift, and because it's a rate every
/// mobile audio API (`AudioTrack`, `AVAudioEngine`) accepts directly. The
/// core itself has no notion of a sample rate — see `gba_core::apu`'s
/// module doc — that's exactly the gap this bridge fills.
const AUDIO_SAMPLE_RATE_HZ: u32 = 32_768;
const CYCLES_PER_AUDIO_SAMPLE: u32 = gba_core::emulator::CPU_CLOCK_HZ / AUDIO_SAMPLE_RATE_HZ;

/// `Apu::sample` outputs small-integer "DAC units" (a single PSG channel
/// tops out at ±15; DMA Direct Sound is a raw signed *byte*, ±127) rather
/// than full-scale 16-bit PCM — matching real GBA hardware, where it's the
/// analog DAC/amp downstream of the chip that turns those into an audible
/// signal. This bridge is that downstream stage: without a gain here, the
/// mix is technically-correct-shaped but sits within ~0.1% of i16's range,
/// i.e. inaudible at normal volume. 128x brings a typical DMA sample
/// (the dominant source in most commercial games' music) up to a healthy
/// portion of full scale.
///
/// The theoretical worst case — both DMA channels at full volume and
/// panned to the same side (±127×2 each) plus all four PSG channels maxed
/// (±15 each, i.e. ±60) — is ±568 DAC units, which 128x pushes to ±72,704:
/// well past i16's ±32,767. That's not actually rare in commercial game
/// music (heavy DMA Direct Sound use is the norm, not the exception), so a
/// hard `.clamp()` there produced audible harsh digital clipping ("popping"
/// during loud passages) instead of the gentle analog saturation real
/// hardware's amp would produce. [`soft_clip`] replaces that hard wall with
/// a smooth tanh saturation curve: for samples well inside range it's
/// indistinguishable from the plain multiply (tanh(x) ≈ x for small x, so
/// normal-volume passages are exactly as loud as before), and it only
/// compresses the rare peaks that would otherwise have clipped, instead of
/// slicing them off abruptly.
const AUDIO_GAIN: f64 = 128.0;

/// Smoothly saturates `x` toward ±`i16::MAX` via `tanh`, instead of the
/// hard-edged distortion a `.clamp()` produces — see [`AUDIO_GAIN`]'s doc
/// comment for why this project needs it at all.
fn soft_clip(x: f64) -> i16 {
    let full_scale = i16::MAX as f64;
    (full_scale * (x / full_scale).tanh()).round() as i16
}

fn scale_to_pcm16(sample: i16) -> i16 {
    soft_clip(sample as f64 * AUDIO_GAIN)
}

/// One emulator session, boxed and handed to the host as an opaque
/// pointer. `None` until a ROM is loaded — the CPU/PPU/Bus don't exist
/// without a cartridge, so there's nothing to step or render yet.
struct Instance {
    emulator: Option<Emulator>,
    /// A user-supplied real BIOS dump, if one was loaded before the ROM —
    /// this project doesn't ship (and can't legally ship) Nintendo's own
    /// BIOS, so [`Instance::load_rom`] falls back to [`Emulator::new`]'s
    /// HLE boot whenever this is `None`.
    bios: Option<Vec<u8>>,
    /// Interleaved (left, right) i16 PCM samples produced by the most
    /// recent [`Self::run_frame`], at [`AUDIO_SAMPLE_RATE_HZ`] — drained by
    /// [`Self::take_audio_samples`] once per host frame, the same way
    /// [`Self::write_framebuffer_argb`] drains the video side.
    audio_buffer: Vec<i16>,
}

impl Instance {
    fn new() -> Self {
        Instance { emulator: None, bios: None, audio_buffer: Vec::new() }
    }

    /// Validates and stores a real BIOS dump for the *next* [`Self::load_rom`]
    /// call to use — it doesn't affect a ROM that's already running (that
    /// `Emulator` was already constructed with whatever boot path was
    /// current at the time). Rejects anything that isn't exactly 16KB,
    /// since that's the one cheap sanity check available without actually
    /// executing the dump.
    fn load_bios(&mut self, bios: Vec<u8>) -> bool {
        if bios.len() != BIOS_SIZE {
            return false;
        }
        self.bios = Some(bios);
        true
    }

    fn load_rom(&mut self, rom: Vec<u8>) -> bool {
        match Cartridge::load(rom) {
            Ok(cartridge) => {
                self.emulator = Some(match &self.bios {
                    Some(bios) => Emulator::new_with_bios(cartridge, bios),
                    None => Emulator::new(cartridge),
                });
                true
            }
            Err(_) => false,
        }
    }

    /// Wires up a hardcoded, colorful Mode 0 scene directly through the
    /// bus (bypassing ROM/CPU execution entirely): a solid blue background
    /// plus a red sprite. Exists so the native rendering pipeline —
    /// Rust PPU -> FFI -> platform Bitmap/Canvas -> screen — can be
    /// verified independently of whether a given ROM's boot sequence ever
    /// reaches the point of drawing anything (the real test ROM doesn't,
    /// without a BIOS). Development/diagnostic only; never used for real
    /// gameplay.
    fn load_test_pattern(&mut self) {
        let rom = vec![0u8; gba_core::cartridge::HEADER_SIZE + 16];
        let cartridge = Cartridge::load(rom).expect("an all-zero synthetic header always parses");
        let mut emulator = Emulator::new(cartridge);

        emulator.bus.write16(0x0400_0000, 0x1100); // DISPCNT: mode 0, BG0 + OBJ enabled
        emulator.bus.write16(0x0400_0008, 0x0000); // BG0CNT: char base 0, screen base 0, 4bpp

        for entry_addr in (0x0600_0000..0x0600_0000 + 32 * 32 * 2).step_by(2) {
            emulator.bus.write16(entry_addr, 0x0001); // every tile map entry -> tile index 1
        }
        for i in 0..32 {
            emulator.bus.write8(0x0600_0020 + i, 0x11); // tile 1: every pixel = color index 1
        }
        emulator.bus.write16(0x0500_0002, 0x7C00); // BG palette color 1 = blue

        emulator.bus.write16(0x0700_0000, 100); // sprite: Y=100
        emulator.bus.write16(0x0700_0002, 108); // X=108 (roughly centered)
        emulator.bus.write16(0x0700_0004, 2); // tile 2, priority 0
        for i in 0..32 {
            emulator.bus.write8(0x0601_0040 + i, 0x22); // OBJ tile 2: every pixel = color index 2
        }
        emulator.bus.write16(0x0500_0204, 0x001F); // OBJ palette color 2 = red

        self.emulator = Some(emulator);
    }

    /// Runs one video frame's worth of cycles, resampling the APU into
    /// [`Self::audio_buffer`] along the way (see [`AUDIO_SAMPLE_RATE_HZ`]).
    /// Runs in [`CYCLES_PER_AUDIO_SAMPLE`]-sized chunks rather than one
    /// `Emulator::run_frame()` call purely so a sample can be taken between
    /// chunks — this doesn't change how much emulation happens, just how
    /// finely the same total cycle budget is sliced. Returns `false` if no
    /// ROM/pattern is loaded, or if the CPU hit an instruction that isn't
    /// implemented yet.
    fn run_frame(&mut self) -> bool {
        let Some(emulator) = &mut self.emulator else { return false };
        self.audio_buffer.clear();
        let mut ran = 0u32;
        while ran < gba_core::emulator::CYCLES_PER_FRAME {
            match emulator.run_cycles(CYCLES_PER_AUDIO_SAMPLE) {
                Ok(actual) => ran += actual,
                Err(_) => return false,
            }
            // `Apu::sample` returns (right, left) — see its doc comment;
            // reordered here to the (left, right) interleaving every
            // stereo PCM consumer (AudioTrack included) expects.
            let (right, left) = emulator.bus.apu().sample();
            self.audio_buffer.push(scale_to_pcm16(left));
            self.audio_buffer.push(scale_to_pcm16(right));
        }
        true
    }

    /// Hands over every PCM sample produced by the most recent
    /// [`Self::run_frame`], clearing the internal buffer — the host is
    /// expected to call this once per video frame, right after
    /// `run_frame`, and feed the result straight to its audio output API.
    fn take_audio_samples(&mut self) -> Vec<i16> {
        std::mem::take(&mut self.audio_buffer)
    }

    /// Writes the current framebuffer as 0xAARRGGBB pixels into `out`
    /// (must be exactly `SCREEN_WIDTH * SCREEN_HEIGHT` long). No-op if no
    /// ROM/pattern is loaded, leaving `out` untouched.
    fn write_framebuffer_argb(&self, out: &mut [i32]) {
        let Some(emulator) = &self.emulator else { return };
        let framebuffer = emulator.bus.ppu().framebuffer();
        for (dst, &color) in out.iter_mut().zip(framebuffer.iter()) {
            let [r, g, b, a] = bgr555_to_rgba8888(color);
            *dst = ((a as i32) << 24) | ((r as i32) << 16) | ((g as i32) << 8) | (b as i32);
        }
    }

    /// The current cartridge's battery-backed save memory (SRAM/Flash), for
    /// the host to persist to its own storage between sessions — this
    /// project has no notion of *where* that goes, only what the bytes
    /// are. Empty if no ROM is loaded or the cartridge has no save chip
    /// (`gba_core`'s `SaveType::None`), which is indistinguishable here
    /// from "nothing to save yet" — both are correctly a no-op to persist.
    fn save_data(&self) -> Vec<u8> {
        self.emulator.as_ref().map_or(Vec::new(), |e| e.bus.cartridge().save_bytes().to_vec())
    }

    /// Restores previously-persisted save memory (from [`Self::save_data`])
    /// into the currently loaded cartridge. Must be called after
    /// [`Self::load_rom`], since loading a ROM starts its save chip blank —
    /// there's no cartridge to restore into before that. A no-op if no ROM
    /// is loaded; a length mismatch (e.g. a save file left over from a
    /// different game) is handled by `gba_core` itself, not here.
    fn load_save_data(&mut self, data: &[u8]) -> bool {
        let Some(emulator) = &mut self.emulator else { return false };
        emulator.bus.cartridge_mut().load_save_bytes(data);
        true
    }

    /// Forwards a discrete button event to the joypad. `key` is the same
    /// numeric ID on both sides of the bridge — see
    /// `gba_core::joypad::Button::from_index`. Unknown IDs and calls before
    /// a ROM/pattern is loaded are silently ignored.
    fn set_key(&mut self, key: i32, pressed: bool) {
        let (Some(button), Some(emulator)) = (gba_core::joypad::Button::from_index(key), &mut self.emulator) else {
            return;
        };
        if pressed {
            emulator.press_key(button);
        } else {
            emulator.release_key(button);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_clip_is_near_identity_for_small_inputs() {
        // tanh(x) ≈ x for small x, so ordinary-volume samples must come out
        // essentially unchanged from a plain multiply — this is what keeps
        // normal-volume passages exactly as loud as before the fix.
        assert_eq!(soft_clip(1000.0), 1000);
        assert_eq!(soft_clip(-1000.0), -1000);
    }

    #[test]
    fn soft_clip_never_overflows_on_extreme_inputs() {
        // A wildly out-of-range input must saturate smoothly toward (not
        // past) i16's bounds — no panic, no wraparound. `tanh` gets close
        // enough to ±1 here that the rounded result lands exactly on
        // i16::MAX/MIN, which is fine — the property that matters is no
        // overflow, not staying strictly inside.
        let huge = soft_clip(1_000_000.0);
        assert!(huge > 0 && huge <= i16::MAX);
        let huge_negative = soft_clip(-1_000_000.0);
        assert!(huge_negative < 0 && huge_negative >= i16::MIN);
    }

    #[test]
    fn scale_to_pcm16_handles_the_theoretical_worst_case_without_panicking() {
        // Both DMA channels maxed and panned together, plus all four PSG
        // channels maxed: ±568 DAC units (see AUDIO_GAIN's doc comment).
        let worst_case = scale_to_pcm16(568);
        assert!(worst_case > 0);
        let worst_case_negative = scale_to_pcm16(-568);
        assert!(worst_case_negative < 0);
    }

    /// Minimal synthetic ROM carrying a save-type signature — same
    /// construction `gba_core::cartridge::cartridge`'s own tests use, just
    /// duplicated here since that helper isn't exported (this crate has no
    /// business depending on `gba_core`'s private test internals).
    fn rom_with_signature(signature: &[u8]) -> Vec<u8> {
        let mut rom = vec![0u8; gba_core::cartridge::HEADER_SIZE + 64];
        rom[0xB2] = 0x96; // fixed value real header parsing checks
        rom[gba_core::cartridge::HEADER_SIZE..gba_core::cartridge::HEADER_SIZE + signature.len()]
            .copy_from_slice(signature);
        rom
    }

    #[test]
    fn save_data_round_trips_through_a_fresh_instance() {
        let mut writer = Instance::new();
        assert!(writer.load_rom(rom_with_signature(b"SRAM_V113")));

        // Nothing saved yet: a cartridge with a save chip starts blank —
        // 0xFF throughout, matching real erased battery-backed SRAM, not 0.
        let blank = writer.save_data();
        assert!(blank.iter().all(|&b| b == 0xFF), "SRAM must start erased (0xFF)");

        // Stand in for the game writing to its own save chip during play.
        writer.emulator.as_mut().unwrap().bus.cartridge_mut().write_save_byte(10, 0x42);
        let saved = writer.save_data();
        assert_eq!(saved[10], 0x42);

        // A brand new Instance (modeling the app being closed and
        // reopened) must come back with that exact save applied after
        // load_rom + load_save_data, matching maybeLoadRom's own sequence
        // on the Kotlin side.
        let mut reader = Instance::new();
        assert!(reader.load_rom(rom_with_signature(b"SRAM_V113")));
        assert!(reader.load_save_data(&saved));
        assert_eq!(reader.save_data(), saved);
    }

    #[test]
    fn save_data_is_empty_for_a_cartridge_without_a_save_chip() {
        let mut instance = Instance::new();
        assert!(instance.load_rom(rom_with_signature(b"nothing here")));
        assert!(instance.save_data().is_empty());
    }

    #[test]
    fn save_data_is_empty_before_any_rom_is_loaded() {
        let instance = Instance::new();
        assert!(instance.save_data().is_empty());
    }

    #[test]
    fn load_save_data_is_a_no_op_before_any_rom_is_loaded() {
        let mut instance = Instance::new();
        assert!(!instance.load_save_data(&[1, 2, 3]));
    }
}
