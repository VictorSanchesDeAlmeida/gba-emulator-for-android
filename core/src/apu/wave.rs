//! GBA sound channel 3: plays back an arbitrary 4-bit waveform stored in
//! "Wave RAM" (0x04000090-0x0400009F) instead of a fixed duty cycle.
//!
//! Simplification: real hardware's 1-bank mode plays from the bank
//! *opposite* the one currently selected for CPU access (so a game can
//! write the next waveform into the inactive bank while the other plays).
//! This implementation instead always plays from whichever bank is
//! selected at trigger time. Games that write a waveform once and just let
//! it loop (by far the common case) are unaffected; the rarer
//! double-buffered-waveform technique is not reproduced faithfully.

const FREQ_TIMER_SCALE: u32 = 8; // (2048-frequency)*8 GBA cycles, GB's "*2" scaled by 4

#[derive(Debug)]
pub(crate) struct WaveChannel {
    dac_power: bool,
    two_banks: bool,
    bank_select: bool,
    wave_ram: [[u8; 16]; 2], // 2 banks * 32 4-bit samples packed 2-per-byte
    length_data: u16,        // 0-255
    volume_code: u8,         // 0-3
    force_75_percent: bool,
    frequency: u16,
    length_enable: bool,

    enabled: bool,
    length_counter: u16,
    freq_timer: u32,
    sample_index: u8, // 0..63, wraps at sample_count()
    playing_bank_start: bool,
}

impl WaveChannel {
    pub(crate) fn new() -> Self {
        WaveChannel {
            dac_power: false,
            two_banks: false,
            bank_select: false,
            wave_ram: [[0; 16]; 2],
            length_data: 0,
            volume_code: 0,
            force_75_percent: false,
            frequency: 0,
            length_enable: false,
            enabled: false,
            length_counter: 0,
            freq_timer: 1,
            sample_index: 0,
            playing_bank_start: false,
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    // --- SOUND3CNT_L (0x70) ---

    pub(crate) fn write_control_l(&mut self, value: u8) {
        self.two_banks = value & 0x20 != 0;
        self.bank_select = value & 0x40 != 0;
        self.dac_power = value & 0x80 != 0;
    }

    pub(crate) fn read_control_l(&self) -> u8 {
        ((self.two_banks as u8) << 5) | ((self.bank_select as u8) << 6) | ((self.dac_power as u8) << 7)
    }

    // --- SOUND3CNT_H (0x72) ---

    pub(crate) fn write_control_h_lo(&mut self, value: u8) {
        self.length_data = value as u16;
    }

    pub(crate) fn write_control_h_hi(&mut self, value: u8) {
        self.volume_code = (value >> 5) & 0x3;
        self.force_75_percent = value & 0x80 != 0;
    }

    pub(crate) fn read_control_h_hi(&self) -> u8 {
        (self.volume_code << 5) | ((self.force_75_percent as u8) << 7)
    }

    // --- SOUND3CNT_X (0x74) ---

    pub(crate) fn write_control_x_lo(&mut self, value: u8) {
        self.frequency = (self.frequency & 0x0700) | value as u16;
    }

    /// Bit7 of this byte is the trigger bit (bit15 of the full register).
    pub(crate) fn write_control_x_hi(&mut self, value: u8) {
        self.frequency = (self.frequency & 0x00FF) | (((value & 0x7) as u16) << 8);
        self.length_enable = value & 0x40 != 0;
        if value & 0x80 != 0 {
            self.trigger();
        }
    }

    pub(crate) fn read_control_x_hi(&self) -> u8 {
        (self.length_enable as u8) << 6
    }

    // --- Wave RAM (0x90-0x9F): CPU access always targets whichever bank
    // `bank_select` currently points at (see the module-level doc comment
    // for the double-buffering nuance this simplifies away).

    pub(crate) fn read_wave_ram(&self, offset: usize) -> u8 {
        self.wave_ram[self.bank_select as usize][offset & 0xF]
    }

    pub(crate) fn write_wave_ram(&mut self, offset: usize, value: u8) {
        self.wave_ram[self.bank_select as usize][offset & 0xF] = value;
    }

    fn sample_count(&self) -> u8 {
        if self.two_banks {
            64
        } else {
            32
        }
    }

    fn sample_at(&self, index: u8) -> u8 {
        let bank = if self.two_banks && index >= 32 {
            !self.playing_bank_start
        } else {
            self.playing_bank_start
        };
        let local = (index % 32) as usize;
        let byte = self.wave_ram[bank as usize][local / 2];
        if local % 2 == 0 {
            byte >> 4 // high nibble plays first
        } else {
            byte & 0xF
        }
    }

    fn period(&self) -> u32 {
        (2048 - self.frequency as u32) * FREQ_TIMER_SCALE
    }

    fn trigger(&mut self) {
        self.enabled = self.dac_power;
        if self.length_counter == 0 {
            self.length_counter = 256 - self.length_data;
        }
        self.freq_timer = self.period();
        self.sample_index = 0;
        self.playing_bank_start = self.bank_select;
    }

    pub(crate) fn clock_length(&mut self) {
        if self.length_enable && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 {
                self.enabled = false;
            }
        }
    }

    pub(crate) fn tick(&mut self, cycles: u32) {
        let mut remaining = cycles;
        while remaining > 0 {
            if self.freq_timer <= remaining {
                remaining -= self.freq_timer;
                self.sample_index = (self.sample_index + 1) % self.sample_count();
                self.freq_timer = self.period().max(1);
            } else {
                self.freq_timer -= remaining;
                remaining = 0;
            }
        }
    }

    /// Instantaneous 0-15 amplitude, already volume-shifted.
    pub(crate) fn amplitude(&self) -> u8 {
        if !self.enabled || !self.dac_power {
            return 0;
        }
        let raw = self.sample_at(self.sample_index);
        if self.force_75_percent {
            (raw as u16 * 3 / 4) as u8
        } else {
            match self.volume_code {
                0 => 0,
                1 => raw,
                2 => raw >> 1,
                _ => raw >> 2,
            }
        }
    }
}

impl Default for WaveChannel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_control_h(ch: &mut WaveChannel, value: u16) {
        ch.write_control_h_lo((value & 0xFF) as u8);
        ch.write_control_h_hi((value >> 8) as u8);
    }

    fn set_control_x(ch: &mut WaveChannel, value: u16) {
        ch.write_control_x_lo((value & 0xFF) as u8);
        ch.write_control_x_hi((value >> 8) as u8);
    }

    #[test]
    fn dac_power_gates_output_regardless_of_trigger() {
        let mut ch = WaveChannel::new();
        ch.write_wave_ram(0, 0xFF); // both nibbles at max
        set_control_h(&mut ch, 1 << 13); // volume 100%
        set_control_x(&mut ch, 0x8000); // trigger without DAC power set
        assert_eq!(ch.amplitude(), 0, "DAC power (SOUND3CNT_L bit7) was never set");

        ch.write_control_l(0x80); // DAC power on
        set_control_x(&mut ch, 0x8000); // re-trigger
        assert_eq!(ch.amplitude(), 15);
    }

    #[test]
    fn volume_shift_halves_and_quarters_the_sample() {
        let mut ch = WaveChannel::new();
        ch.write_control_l(0x80);
        ch.write_wave_ram(0, 0xFF);
        set_control_h(&mut ch, 2 << 13); // 50%
        set_control_x(&mut ch, 0x8000);
        assert_eq!(ch.amplitude(), 7);

        set_control_h(&mut ch, 3 << 13); // 25%
        set_control_x(&mut ch, 0x8000);
        assert_eq!(ch.amplitude(), 3);
    }

    #[test]
    fn samples_advance_high_nibble_first_then_low_nibble() {
        let mut ch = WaveChannel::new();
        ch.write_control_l(0x80);
        ch.write_wave_ram(0, 0xA5); // high nibble 0xA, low nibble 0x5
        set_control_h(&mut ch, 1 << 13); // 100%
        set_control_x(&mut ch, 0x8000 | 2000); // trigger, high frequency = short period

        assert_eq!(ch.amplitude(), 0xA);
        ch.tick(ch.period());
        assert_eq!(ch.amplitude(), 0x5);
    }

    #[test]
    fn wave_ram_writes_target_the_selected_bank() {
        let mut ch = WaveChannel::new();
        ch.write_control_l(0x00); // bank_select=0
        ch.write_wave_ram(0, 0x11);
        ch.write_control_l(0x40); // bank_select=1
        ch.write_wave_ram(0, 0x22);

        ch.write_control_l(0x00);
        assert_eq!(ch.read_wave_ram(0), 0x11);
        ch.write_control_l(0x40);
        assert_eq!(ch.read_wave_ram(0), 0x22);
    }

    #[test]
    fn length_counter_disables_the_channel_at_zero() {
        let mut ch = WaveChannel::new();
        ch.write_control_l(0x80);
        set_control_h(&mut ch, 255); // length_data=255 -> counter starts at 1
        set_control_x(&mut ch, 0x8000 | 0x4000); // trigger + length_enable
        assert!(ch.enabled());
        ch.clock_length();
        assert!(!ch.enabled());
    }
}
