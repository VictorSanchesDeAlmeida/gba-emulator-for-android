//! GBA sound channel 4: a pseudo-random noise generator driven by a linear
//! feedback shift register (LFSR), with the same length/envelope hardware
//! as channels 1/2.

/// GB "dividing ratio" table, in units of the GB's own 4.19MHz clock;
/// scaled by 4 (see `pulse.rs`'s module comment) to land in GBA cycles.
const DIVISOR_TABLE: [u32; 8] = [8, 16, 32, 48, 64, 80, 96, 112];

#[derive(Debug)]
pub(crate) struct NoiseChannel {
    length_data: u8,
    envelope_initial_volume: u8,
    envelope_increasing: bool,
    envelope_period: u8,
    divisor_code: u8,
    width_7bit: bool,
    shift: u8,
    length_enable: bool,

    enabled: bool,
    length_counter: u16,
    volume: u8,
    envelope_timer: u8,
    freq_timer: u32,
    lfsr: u16,
}

impl NoiseChannel {
    pub(crate) fn new() -> Self {
        NoiseChannel {
            length_data: 0,
            envelope_initial_volume: 0,
            envelope_increasing: false,
            envelope_period: 0,
            divisor_code: 0,
            width_7bit: false,
            shift: 0,
            length_enable: false,
            enabled: false,
            length_counter: 0,
            volume: 0,
            envelope_timer: 0,
            freq_timer: 1,
            lfsr: 0x7FFF,
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    // --- SOUND4CNT_L (0x78) ---

    pub(crate) fn write_length_envelope_lo(&mut self, value: u8) {
        self.length_data = value & 0x3F;
    }

    pub(crate) fn write_length_envelope_hi(&mut self, value: u8) {
        self.envelope_period = value & 0x7;
        self.envelope_increasing = value & 0x08 != 0;
        self.envelope_initial_volume = (value >> 4) & 0xF;
    }

    pub(crate) fn read_length_envelope_hi(&self) -> u8 {
        self.envelope_period | ((self.envelope_increasing as u8) << 3) | (self.envelope_initial_volume << 4)
    }

    // --- SOUND4CNT_H (0x7C) ---

    pub(crate) fn write_control_lo(&mut self, value: u8) {
        self.divisor_code = value & 0x7;
        self.width_7bit = value & 0x08 != 0;
        self.shift = (value >> 4) & 0xF;
    }

    pub(crate) fn read_control_lo(&self) -> u8 {
        self.divisor_code | ((self.width_7bit as u8) << 3) | (self.shift << 4)
    }

    /// Bit7 of this byte is the trigger bit (bit15 of the full register).
    pub(crate) fn write_control_hi(&mut self, value: u8) {
        self.length_enable = value & 0x40 != 0;
        if value & 0x80 != 0 {
            self.trigger();
        }
    }

    pub(crate) fn read_control_hi(&self) -> u8 {
        (self.length_enable as u8) << 6
    }

    fn period(&self) -> u32 {
        (DIVISOR_TABLE[self.divisor_code as usize] << self.shift) * 4
    }

    fn trigger(&mut self) {
        self.enabled = true;
        if self.length_counter == 0 {
            self.length_counter = 64 - self.length_data as u16;
        }
        self.freq_timer = self.period();
        self.volume = self.envelope_initial_volume;
        self.envelope_timer = if self.envelope_period == 0 { 8 } else { self.envelope_period };
        self.lfsr = 0x7FFF;
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

    fn step_lfsr(&mut self) {
        let xor_bit = (self.lfsr & 1) ^ ((self.lfsr >> 1) & 1);
        self.lfsr = (self.lfsr >> 1) | (xor_bit << 14);
        if self.width_7bit {
            self.lfsr = (self.lfsr & !(1 << 6)) | (xor_bit << 6);
        }
    }

    pub(crate) fn tick(&mut self, cycles: u32) {
        let mut remaining = cycles;
        while remaining > 0 {
            if self.freq_timer <= remaining {
                remaining -= self.freq_timer;
                self.step_lfsr();
                self.freq_timer = self.period().max(1);
            } else {
                self.freq_timer -= remaining;
                remaining = 0;
            }
        }
    }

    /// Instantaneous 0-15 amplitude: the current volume when the LFSR's
    /// bit 0 is clear, silent when it's set.
    pub(crate) fn amplitude(&self) -> u8 {
        if !self.enabled || self.lfsr & 1 != 0 {
            0
        } else {
            self.volume
        }
    }
}

impl Default for NoiseChannel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_length_envelope(ch: &mut NoiseChannel, value: u16) {
        ch.write_length_envelope_lo((value & 0xFF) as u8);
        ch.write_length_envelope_hi((value >> 8) as u8);
    }

    fn set_control(ch: &mut NoiseChannel, value: u16) {
        ch.write_control_lo((value & 0xFF) as u8);
        ch.write_control_hi((value >> 8) as u8);
    }

    #[test]
    fn trigger_resets_the_lfsr_to_all_ones() {
        let mut ch = NoiseChannel::new();
        ch.lfsr = 0x0001;
        set_control(&mut ch, 0x8000);
        assert_eq!(ch.lfsr, 0x7FFF);
    }

    #[test]
    fn fifteen_bit_lfsr_first_step_from_all_ones_is_deterministic() {
        let mut ch = NoiseChannel::new();
        set_control(&mut ch, 0x8000); // trigger, 15-bit width, divisor_code=0, shift=0
        set_length_envelope(&mut ch, 15 << 12); // volume=15 so amplitude reflects the LFSR bit directly

        // lfsr=0x7FFF: bit0=1, bit1=1, xor=0 -> shifted right with a 0 fed into bit14.
        ch.tick(ch.period());
        assert_eq!(ch.lfsr, 0x3FFF);
        assert_eq!(ch.amplitude(), 0, "bit0 is now 1 (0x3FFF is odd), so output must be silent");
    }

    #[test]
    fn seven_bit_mode_also_feeds_the_xor_result_into_bit_6() {
        let mut ch = NoiseChannel::new();
        set_control(&mut ch, 0x8000 | 0x08); // trigger, 7-bit width
        ch.tick(ch.period());
        assert_eq!(ch.lfsr & (1 << 6), 0, "xor result (0) must also land in bit6 for 7-bit mode");
    }

    #[test]
    fn length_counter_disables_the_channel_at_zero() {
        let mut ch = NoiseChannel::new();
        set_length_envelope(&mut ch, 63); // length_data=63 -> counter starts at 1
        set_control(&mut ch, 0x8000 | 0x4000); // trigger + length_enable
        assert!(ch.enabled());
        ch.clock_length();
        assert!(!ch.enabled());
    }

    #[test]
    fn envelope_decreases_when_configured_to() {
        let mut ch = NoiseChannel::new();
        set_length_envelope(&mut ch, (1 << 8) | (5 << 12)); // envelope_period=1, decreasing, initial vol=5
        set_control(&mut ch, 0x8000);
        assert_eq!(ch.volume, 5);
        ch.clock_envelope();
        assert_eq!(ch.volume, 4);
    }
}
