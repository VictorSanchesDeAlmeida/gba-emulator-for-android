//! The GBA APU: 4 legacy PSG channels inherited from the DMG/CGB (pulse
//! with sweep, pulse, wave, noise) plus 2 GBA-specific Direct Sound (DMA)
//! channels, mixed into a single stereo output sample.
//!
//! Scope for this etapa: every channel's register model and waveform
//! generation is implemented and unit-tested (see `pulse.rs`/`wave.rs`/
//! `noise.rs`/`dma_sound.rs`), and [`Apu::sample`] produces a correct
//! instantaneous mixed stereo sample from live channel state. What's
//! *not* here yet: any actual audio output. This module has no concept of
//! an output sample rate or a ring buffer — like the PPU produces a
//! framebuffer for the native bridge to blit, the APU produces a sample on
//! demand for a future native bridge to resample/buffer into whatever the
//! platform's audio API wants. There is also no general DMA controller in
//! this emulator yet, so DMA Sound's FIFOs are fed by direct CPU writes to
//! FIFO_A/FIFO_B rather than by an actual DMA1/2 transfer.

mod dma_sound;
mod noise;
mod pulse;
mod wave;

use dma_sound::DmaSoundChannel;
use noise::NoiseChannel;
use pulse::PulseChannel;
use wave::WaveChannel;

/// I/O block this module owns: SOUND1CNT_L..FIFO_B (0x04000060..0x040000A8).
pub const IO_BASE: usize = 0x60;
pub(crate) const IO_SIZE: usize = 0x48;

/// The frame sequencer clocks length/envelope/sweep at 512 Hz, a fixed
/// fraction of the CPU clock independent of any channel's own frequency.
const FRAME_SEQUENCER_PERIOD: u32 = crate::emulator::CPU_CLOCK_HZ / 512;

#[derive(Debug)]
pub struct Apu {
    pulse1: PulseChannel,
    pulse2: PulseChannel,
    wave: WaveChannel,
    noise: NoiseChannel,
    dma_a: DmaSoundChannel,
    dma_b: DmaSoundChannel,

    soundcnt_l: u16,
    soundcnt_h: u16,
    master_enable: bool,
    soundbias: u16,

    frame_sequencer_timer: u32,
    frame_sequencer_step: u8,
}

impl Apu {
    pub fn new() -> Self {
        Apu {
            pulse1: PulseChannel::new(true),
            pulse2: PulseChannel::new(false),
            wave: WaveChannel::new(),
            noise: NoiseChannel::new(),
            dma_a: DmaSoundChannel::new(),
            dma_b: DmaSoundChannel::new(),
            soundcnt_l: 0,
            soundcnt_h: 0,
            master_enable: false,
            soundbias: 0,
            frame_sequencer_timer: 0,
            frame_sequencer_step: 0,
        }
    }

    pub(crate) fn read_register(&self, offset: usize) -> u8 {
        match offset {
            0x00 => self.pulse1.read_sweep(),
            0x02 => self.pulse1.read_length_envelope_lo(),
            0x03 => self.pulse1.read_length_envelope_hi(),
            0x05 => self.pulse1.read_freq_hi(),
            0x08 => self.pulse2.read_length_envelope_lo(),
            0x09 => self.pulse2.read_length_envelope_hi(),
            0x0D => self.pulse2.read_freq_hi(),
            0x10 => self.wave.read_control_l(),
            0x13 => self.wave.read_control_h_hi(),
            0x15 => self.wave.read_control_x_hi(),
            0x19 => self.noise.read_length_envelope_hi(),
            0x1C => self.noise.read_control_lo(),
            0x1D => self.noise.read_control_hi(),
            0x20 => (self.soundcnt_l & 0xFF) as u8,
            0x21 => (self.soundcnt_l >> 8) as u8,
            0x22 => (self.soundcnt_h & 0xFF) as u8,
            0x23 => (self.soundcnt_h >> 8) as u8,
            0x24 => self.channel_status(),
            0x28 => (self.soundbias & 0xFF) as u8,
            0x29 => (self.soundbias >> 8) as u8,
            0x30..=0x3F => self.wave.read_wave_ram(offset - 0x30),
            _ => 0,
        }
    }

    pub(crate) fn write_register(&mut self, offset: usize, value: u8) {
        match offset {
            0x00 => self.pulse1.write_sweep(value),
            0x02 => self.pulse1.write_length_envelope_lo(value),
            0x03 => self.pulse1.write_length_envelope_hi(value),
            0x04 => self.pulse1.write_freq_lo(value),
            0x05 => self.pulse1.write_freq_hi(value),
            0x08 => self.pulse2.write_length_envelope_lo(value),
            0x09 => self.pulse2.write_length_envelope_hi(value),
            0x0C => self.pulse2.write_freq_lo(value),
            0x0D => self.pulse2.write_freq_hi(value),
            0x10 => self.wave.write_control_l(value),
            0x12 => self.wave.write_control_h_lo(value),
            0x13 => self.wave.write_control_h_hi(value),
            0x14 => self.wave.write_control_x_lo(value),
            0x15 => self.wave.write_control_x_hi(value),
            0x18 => self.noise.write_length_envelope_lo(value),
            0x19 => self.noise.write_length_envelope_hi(value),
            0x1C => self.noise.write_control_lo(value),
            0x1D => self.noise.write_control_hi(value),
            0x20 => self.soundcnt_l = (self.soundcnt_l & 0xFF00) | value as u16,
            0x21 => self.soundcnt_l = (self.soundcnt_l & 0x00FF) | ((value as u16) << 8),
            0x22 => self.soundcnt_h = (self.soundcnt_h & 0xFF00) | value as u16,
            0x23 => {
                self.soundcnt_h = (self.soundcnt_h & 0x00FF) | ((value as u16) << 8);
                if value & 0x08 != 0 {
                    self.dma_a.reset();
                }
                if value & 0x80 != 0 {
                    self.dma_b.reset();
                }
            }
            0x24 => self.master_enable = value & 0x80 != 0,
            0x28 => self.soundbias = (self.soundbias & 0xFF00) | value as u16,
            0x29 => self.soundbias = (self.soundbias & 0x00FF) | ((value as u16) << 8),
            0x30..=0x3F => self.wave.write_wave_ram(offset - 0x30, value),
            0x40..=0x43 => self.dma_a.push(value),
            0x44..=0x47 => self.dma_b.push(value),
            _ => {}
        }
    }

    fn channel_status(&self) -> u8 {
        (self.pulse1.enabled() as u8)
            | ((self.pulse2.enabled() as u8) << 1)
            | ((self.wave.enabled() as u8) << 2)
            | ((self.noise.enabled() as u8) << 3)
            | ((self.master_enable as u8) << 7)
    }

    /// Advances every channel and the frame sequencer by `cycles`, and
    /// pops a DMA sound sample for A/B if the timer each is routed to
    /// (SOUNDCNT_H bits 10/14) is in `timer_overflowed` (from
    /// [`crate::timer::Timers::tick`]'s per-tick result).
    pub fn tick(&mut self, cycles: u32, timer_overflowed: [bool; 4]) {
        if !self.master_enable {
            return;
        }

        self.pulse1.tick(cycles);
        self.pulse2.tick(cycles);
        self.wave.tick(cycles);
        self.noise.tick(cycles);

        self.frame_sequencer_timer += cycles;
        while self.frame_sequencer_timer >= FRAME_SEQUENCER_PERIOD {
            self.frame_sequencer_timer -= FRAME_SEQUENCER_PERIOD;
            self.clock_frame_sequencer();
        }

        let dma_a_timer = if self.soundcnt_h & 0x0400 != 0 { 1 } else { 0 };
        let dma_b_timer = if self.soundcnt_h & 0x4000 != 0 { 1 } else { 0 };
        if timer_overflowed[dma_a_timer] {
            self.dma_a.pop();
        }
        if timer_overflowed[dma_b_timer] {
            self.dma_b.pop();
        }
    }

    fn clock_frame_sequencer(&mut self) {
        match self.frame_sequencer_step {
            0 | 4 => self.clock_length(),
            2 | 6 => {
                self.clock_length();
                self.pulse1.clock_sweep();
            }
            7 => {
                self.pulse1.clock_envelope();
                self.pulse2.clock_envelope();
                self.noise.clock_envelope();
            }
            _ => {}
        }
        self.frame_sequencer_step = (self.frame_sequencer_step + 1) % 8;
    }

    fn clock_length(&mut self) {
        self.pulse1.clock_length();
        self.pulse2.clock_length();
        self.wave.clock_length();
        self.noise.clock_length();
    }

    /// Converts a 0-15 PSG amplitude into a signed "DAC" sample, the way
    /// real GB/GBA sound hardware's analog output is centered — including
    /// the fact that a "low" waveform sample (amplitude 0 while playing)
    /// is a negative excursion, not silence. A channel that isn't playing
    /// at all contributes flat silence instead: without gating on
    /// `active`, an idle-but-routed channel would inject a constant -15
    /// DC offset into every mix, which real hardware's DAC-off state does
    /// not reproduce closely enough to be worth modeling here.
    fn dac_contribution(amplitude: u8, active: bool) -> i32 {
        if active {
            amplitude as i32 * 2 - 15
        } else {
            0
        }
    }

    /// The current instantaneous mixed stereo sample. Computed on demand
    /// from live channel state — see the module doc comment for why there
    /// is no buffering/resampling here.
    pub fn sample(&self) -> (i16, i16) {
        if !self.master_enable {
            return (0, 0);
        }

        let psg = [
            (Self::dac_contribution(self.pulse1.amplitude(), self.pulse1.enabled()), 0),
            (Self::dac_contribution(self.pulse2.amplitude(), self.pulse2.enabled()), 1),
            (Self::dac_contribution(self.wave.amplitude(), self.wave.enabled()), 2),
            (Self::dac_contribution(self.noise.amplitude(), self.noise.enabled()), 3),
        ];
        let mut psg_right = 0i32;
        let mut psg_left = 0i32;
        for &(sample, ch) in &psg {
            if self.soundcnt_l & (0x0100 << ch) != 0 {
                psg_right += sample;
            }
            if self.soundcnt_l & (0x1000 << ch) != 0 {
                psg_left += sample;
            }
        }
        let master_vol_right = (self.soundcnt_l & 0x7) as i32 + 1;
        let master_vol_left = ((self.soundcnt_l >> 4) & 0x7) as i32 + 1;
        psg_right = psg_right * master_vol_right / 8;
        psg_left = psg_left * master_vol_left / 8;

        // PSG output ratio (SOUNDCNT_H bits 0-1): 25/50/100%. Code 3 is
        // documented as "forbidden" and behaves like 100% on real hardware.
        let (num, den) = match self.soundcnt_h & 0x3 {
            0 => (1, 4),
            1 => (1, 2),
            _ => (1, 1),
        };
        psg_right = psg_right * num / den;
        psg_left = psg_left * num / den;

        let dma_a_sample = self.dma_a.sample() as i32 * if self.soundcnt_h & 0x04 != 0 { 2 } else { 1 };
        let dma_b_sample = self.dma_b.sample() as i32 * if self.soundcnt_h & 0x08 != 0 { 2 } else { 1 };

        let mut right = psg_right;
        let mut left = psg_left;
        if self.soundcnt_h & 0x0100 != 0 {
            right += dma_a_sample;
        }
        if self.soundcnt_h & 0x0200 != 0 {
            left += dma_a_sample;
        }
        if self.soundcnt_h & 0x1000 != 0 {
            right += dma_b_sample;
        }
        if self.soundcnt_h & 0x2000 != 0 {
            left += dma_b_sample;
        }

        (right.clamp(i16::MIN as i32, i16::MAX as i32) as i16, left.clamp(i16::MIN as i32, i16::MAX as i32) as i16)
    }
}

impl Default for Apu {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled_apu() -> Apu {
        let mut apu = Apu::new();
        apu.write_register(0x24, 0x80); // master enable
        apu
    }

    #[test]
    fn master_disable_silences_everything() {
        let apu = Apu::new(); // master_enable defaults to false
        assert_eq!(apu.sample(), (0, 0));
    }

    #[test]
    fn channel_status_reflects_which_psg_channels_are_playing() {
        let mut apu = enabled_apu();
        assert_eq!(apu.read_register(0x24) & 0x0F, 0, "nothing triggered yet");

        apu.write_register(0x05, 0x80); // pulse1 trigger
        assert_eq!(apu.read_register(0x24) & 0x0F, 0x01);
    }

    #[test]
    fn pulse1_triggered_with_full_volume_and_pan_produces_a_nonzero_sample() {
        let mut apu = enabled_apu();
        apu.write_register(0x20, 0x77); // SOUNDCNT_L: max master volume both sides
        apu.write_register(0x21, 0x11); // pulse1 routed to both L and R
        apu.write_register(0x22, 0x02); // SOUNDCNT_H: PSG ratio = 100%
        apu.write_register(0x02, 0x40); // duty=1
        apu.write_register(0x03, 0xF0); // initial volume=15
        apu.write_register(0x04, 0x00);
        apu.write_register(0x05, 0x80); // trigger, frequency=0

        let (r, l) = apu.sample();
        assert!(r != 0 && l != 0, "a fully configured, triggered channel must produce sound");
        assert_eq!(r, l, "panned equally to both sides");
    }

    #[test]
    fn dma_sound_a_mixes_into_the_right_channel_when_only_right_is_enabled() {
        let mut apu = enabled_apu();
        apu.write_register(0x22, 0x04); // SOUNDCNT_H lo: DMA A 100% volume
        apu.write_register(0x23, 0x01); // SOUNDCNT_H hi: DMA A right-enabled only (bit8 overall)
        apu.dma_a.push(50);
        apu.dma_a.pop();

        let (r, l) = apu.sample();
        assert!(r > 0);
        assert_eq!(l, 0);
    }

    #[test]
    fn wave_ram_is_reachable_through_the_apu_register_dispatch() {
        let mut apu = Apu::new();
        apu.write_register(0x30, 0xAB);
        assert_eq!(apu.read_register(0x30), 0xAB);
    }

    #[test]
    fn fifo_a_and_b_push_through_distinct_offsets() {
        let mut apu = Apu::new();
        apu.write_register(0x40, 11);
        apu.write_register(0x44, 22);
        apu.dma_a.pop();
        apu.dma_b.pop();
        assert_eq!(apu.dma_a.sample(), 11);
        assert_eq!(apu.dma_b.sample(), 22);
    }

    #[test]
    fn frame_sequencer_clocks_length_at_the_documented_cadence() {
        let mut apu = enabled_apu();
        apu.write_register(0x02, 63); // pulse1 length_data=63 -> counter starts at 1
        apu.write_register(0x05, 0xC0); // trigger + length_enable
        assert!(apu.pulse1.enabled());

        apu.tick(FRAME_SEQUENCER_PERIOD, [false; 4]); // one frame-sequencer step: step 0 clocks length
        assert!(!apu.pulse1.enabled(), "one length clock must have exhausted the 1-tick counter");
    }
}
