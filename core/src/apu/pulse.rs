//! Shared logic for GBA sound channels 1 and 2: a square wave at one of 4
//! duty cycles, with a volume envelope and (channel 1 only) a frequency
//! sweep. This is the same hardware design the DMG/CGB used — the GBA just
//! clocks it from the CPU's 16.78MHz clock instead of the GB's 4.19MHz one
//! (exactly 4x), so every GB-documented cycle count here is multiplied by 4.

/// Duty-cycle waveforms, one bit per of the 8 steps in a cycle (Pan Docs
/// "Sound Channel 1/2 - Tone"): 12.5%, 25%, 50%, 75%.
const DUTY_TABLE: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 1, 1, 1],
    [0, 1, 1, 1, 1, 1, 1, 0],
];

#[derive(Debug)]
pub(crate) struct PulseChannel {
    has_sweep: bool,

    // Raw register fields, kept exactly as written for register readback.
    sweep_period: u8,
    sweep_negate: bool,
    sweep_shift: u8,
    duty: u8,
    length_data: u8,
    envelope_initial_volume: u8,
    envelope_increasing: bool,
    envelope_period: u8,
    frequency: u16,
    length_enable: bool,

    // Runtime state, derived from the registers at trigger time and then
    // evolving independently as the channel plays.
    enabled: bool,
    length_counter: u16,
    volume: u8,
    envelope_timer: u8,
    freq_timer: u32,
    duty_step: u8,
    sweep_timer: u8,
    sweep_enabled: bool,
    sweep_shadow_freq: u16,
}

impl PulseChannel {
    pub(crate) fn new(has_sweep: bool) -> Self {
        PulseChannel {
            has_sweep,
            sweep_period: 0,
            sweep_negate: false,
            sweep_shift: 0,
            duty: 0,
            length_data: 0,
            envelope_initial_volume: 0,
            envelope_increasing: false,
            envelope_period: 0,
            frequency: 0,
            length_enable: false,
            enabled: false,
            length_counter: 0,
            volume: 0,
            envelope_timer: 0,
            freq_timer: 1,
            duty_step: 0,
            sweep_timer: 0,
            sweep_enabled: false,
            sweep_shadow_freq: 0,
        }
    }

    // --- Register writes (SOUND1CNT_L/H/X or SOUND2CNT_L/H) ---

    pub(crate) fn write_sweep(&mut self, value: u8) {
        self.sweep_shift = value & 0x7;
        self.sweep_negate = value & 0x08 != 0;
        self.sweep_period = (value >> 4) & 0x7;
    }

    pub(crate) fn read_sweep(&self) -> u8 {
        self.sweep_shift | ((self.sweep_negate as u8) << 3) | (self.sweep_period << 4)
    }

    pub(crate) fn write_length_envelope_lo(&mut self, value: u8) {
        self.length_data = value & 0x3F;
        self.duty = (value >> 6) & 0x3;
    }

    pub(crate) fn write_length_envelope_hi(&mut self, value: u8) {
        self.envelope_period = value & 0x7;
        self.envelope_increasing = value & 0x08 != 0;
        self.envelope_initial_volume = (value >> 4) & 0xF;
    }

    pub(crate) fn read_length_envelope_lo(&self) -> u8 {
        self.duty << 6
    }

    pub(crate) fn read_length_envelope_hi(&self) -> u8 {
        self.envelope_period | ((self.envelope_increasing as u8) << 3) | (self.envelope_initial_volume << 4)
    }

    pub(crate) fn write_freq_lo(&mut self, value: u8) {
        self.frequency = (self.frequency & 0x0700) | value as u16;
    }

    /// Bit7 of this byte is the trigger bit (bit15 of the full 16-bit
    /// register) — the only byte write that can restart the channel.
    pub(crate) fn write_freq_hi(&mut self, value: u8) {
        self.frequency = (self.frequency & 0x00FF) | (((value & 0x7) as u16) << 8);
        self.length_enable = value & 0x40 != 0;
        if value & 0x80 != 0 {
            self.trigger();
        }
    }

    pub(crate) fn read_freq_hi(&self) -> u8 {
        (self.length_enable as u8) << 6
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    fn period(&self) -> u32 {
        (2048 - self.frequency as u32) * 16
    }

    fn trigger(&mut self) {
        self.enabled = true;
        if self.length_counter == 0 {
            self.length_counter = 64 - self.length_data as u16;
        }
        self.freq_timer = self.period();
        self.volume = self.envelope_initial_volume;
        self.envelope_timer = if self.envelope_period == 0 { 8 } else { self.envelope_period };

        if self.has_sweep {
            self.sweep_shadow_freq = self.frequency;
            self.sweep_timer = if self.sweep_period == 0 { 8 } else { self.sweep_period };
            self.sweep_enabled = self.sweep_period != 0 || self.sweep_shift != 0;
            if self.sweep_shift != 0 {
                self.compute_sweep_frequency();
            }
        }
    }

    /// Computes the sweep-shifted frequency and disables the channel if it
    /// overflows past 11 bits — a real DMG quirk that also applies on the
    /// immediate check triggered by [`Self::trigger`].
    fn compute_sweep_frequency(&mut self) -> u16 {
        let delta = self.sweep_shadow_freq >> self.sweep_shift;
        let new_freq = if self.sweep_negate {
            self.sweep_shadow_freq.wrapping_sub(delta)
        } else {
            self.sweep_shadow_freq.wrapping_add(delta)
        };
        if new_freq > 2047 {
            self.enabled = false;
        }
        new_freq
    }

    pub(crate) fn clock_sweep(&mut self) {
        if !self.has_sweep || !self.sweep_enabled {
            return;
        }
        if self.sweep_timer > 0 {
            self.sweep_timer -= 1;
        }
        if self.sweep_timer != 0 {
            return;
        }
        self.sweep_timer = if self.sweep_period == 0 { 8 } else { self.sweep_period };
        if self.sweep_period == 0 {
            return;
        }
        let new_freq = self.compute_sweep_frequency();
        if new_freq <= 2047 && self.sweep_shift != 0 {
            self.sweep_shadow_freq = new_freq;
            self.frequency = new_freq;
            self.compute_sweep_frequency(); // hardware re-checks overflow immediately after writing back
        }
    }

    pub(crate) fn clock_length(&mut self) {
        if self.length_enable && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 {
                self.enabled = false;
            }
        }
    }

    pub(crate) fn clock_envelope(&mut self) {
        if self.envelope_period == 0 {
            return;
        }
        if self.envelope_timer > 0 {
            self.envelope_timer -= 1;
        }
        if self.envelope_timer == 0 {
            self.envelope_timer = self.envelope_period;
            if self.envelope_increasing && self.volume < 15 {
                self.volume += 1;
            } else if !self.envelope_increasing && self.volume > 0 {
                self.volume -= 1;
            }
        }
    }

    pub(crate) fn tick(&mut self, cycles: u32) {
        let mut remaining = cycles;
        while remaining > 0 {
            if self.freq_timer <= remaining {
                remaining -= self.freq_timer;
                self.duty_step = (self.duty_step + 1) % 8;
                self.freq_timer = self.period().max(1);
            } else {
                self.freq_timer -= remaining;
                remaining = 0;
            }
        }
    }

    /// Instantaneous 0-15 amplitude ("DAC input"), 0 while the channel is
    /// silent (disabled, or the current duty step is low).
    pub(crate) fn amplitude(&self) -> u8 {
        if !self.enabled || DUTY_TABLE[self.duty as usize][self.duty_step as usize] == 0 {
            0
        } else {
            self.volume
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test convenience: writes both bytes of SOUND1CNT_H/SOUND2CNT_L.
    fn set_length_envelope(ch: &mut PulseChannel, value: u16) {
        ch.write_length_envelope_lo((value & 0xFF) as u8);
        ch.write_length_envelope_hi((value >> 8) as u8);
    }

    /// Test convenience: writes both bytes of SOUND1CNT_X/SOUND2CNT_H. The
    /// hi byte (which carries the trigger bit) is always written last,
    /// matching a real STRH of the whole halfword.
    fn set_freq_control(ch: &mut PulseChannel, value: u16) {
        ch.write_freq_lo((value & 0xFF) as u8);
        ch.write_freq_hi((value >> 8) as u8);
    }

    #[test]
    fn trigger_enables_the_channel_and_loads_length() {
        let mut ch = PulseChannel::new(false);
        set_length_envelope(&mut ch, 63); // length_data=63, duty irrelevant here
        set_freq_control(&mut ch, 0x8000); // trigger
        assert!(ch.enabled());
    }

    #[test]
    fn duty_25_percent_matches_the_documented_pattern() {
        let mut ch = PulseChannel::new(false);
        set_length_envelope(&mut ch, (1 << 6) | (15 << 12)); // duty=1 (25%), initial volume=15
        set_freq_control(&mut ch, 0x8000 | 100); // trigger, frequency=100

        let mut high_steps = 0;
        for _ in 0..8 {
            if ch.amplitude() > 0 {
                high_steps += 1;
            }
            ch.tick(ch.period());
        }
        assert_eq!(high_steps, 2, "25% duty must be high for 2 of 8 steps");
    }

    #[test]
    fn envelope_increases_volume_over_time_when_configured_to() {
        let mut ch = PulseChannel::new(false);
        // initial volume=0, increasing, envelope period=1
        set_length_envelope(&mut ch, 0x0800 | 0x0100);
        set_freq_control(&mut ch, 0x8000 | 100);
        assert_eq!(ch.volume, 0);

        ch.clock_envelope();
        assert_eq!(ch.volume, 1);
        ch.clock_envelope();
        assert_eq!(ch.volume, 2);
    }

    #[test]
    fn envelope_period_zero_never_changes_volume() {
        let mut ch = PulseChannel::new(false);
        set_length_envelope(&mut ch, 0x0800 | (5 << 12)); // envelope_period=0, increasing, initial vol=5
        set_freq_control(&mut ch, 0x8000);
        ch.clock_envelope();
        ch.clock_envelope();
        assert_eq!(ch.volume, 5);
    }

    #[test]
    fn length_counter_disables_the_channel_when_it_reaches_zero() {
        let mut ch = PulseChannel::new(false);
        set_length_envelope(&mut ch, 63); // length_data=63 -> counter starts at 1
        set_freq_control(&mut ch, 0x8000 | 0x4000); // trigger + length_enable
        assert!(ch.enabled());
        ch.clock_length();
        assert!(!ch.enabled(), "length_counter (64-63=1) must hit zero after one clock");
    }

    #[test]
    fn length_disabled_never_stops_the_channel() {
        let mut ch = PulseChannel::new(false);
        set_length_envelope(&mut ch, 63);
        set_freq_control(&mut ch, 0x8000); // trigger, length_enable NOT set
        for _ in 0..10 {
            ch.clock_length();
        }
        assert!(ch.enabled());
    }

    #[test]
    fn sweep_shifts_frequency_up_when_not_negated() {
        let mut ch = PulseChannel::new(true);
        ch.write_sweep(1 | (1 << 4)); // shift=1, negate=false, period=1
        set_freq_control(&mut ch, 0x8000 | 100); // trigger, frequency=100
        ch.clock_sweep(); // sweep_timer counts down from 1 to 0, triggering the update
        assert_eq!(ch.frequency, 100 + (100 >> 1));
    }

    #[test]
    fn sweep_overflow_disables_the_channel_immediately_at_trigger() {
        let mut ch = PulseChannel::new(true);
        ch.write_sweep(1 | (1 << 4)); // shift=1, negate=false, period=1
        set_freq_control(&mut ch, 0x8000 | 2040); // 2040 + (2040>>1) = 3060, past the 2047 ceiling
        assert!(!ch.enabled(), "trigger must run the overflow check immediately, not wait for a sweep clock");
    }

    #[test]
    fn channel_2_ignores_sweep_writes_entirely() {
        let mut ch = PulseChannel::new(false);
        ch.write_sweep(0b0_111_111); // would be a huge sweep if it had one
        set_freq_control(&mut ch, 0x8000 | 100);
        ch.clock_sweep();
        assert_eq!(ch.frequency, 100, "channel 2 has no sweep hardware");
    }
}
