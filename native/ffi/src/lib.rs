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
/// portion of full scale while leaving headroom before every channel maxed
/// out simultaneously (a rare, real-hardware-saturating case) hard-clips.
const AUDIO_GAIN: i32 = 128;

fn scale_to_pcm16(sample: i16) -> i16 {
    (sample as i32 * AUDIO_GAIN).clamp(i16::MIN as i32, i16::MAX as i32) as i16
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
