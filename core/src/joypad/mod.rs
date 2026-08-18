//! The GBA joypad: 10 buttons (D-Pad, A, B, Select, Start, L, R) exposed
//! through KEYINPUT/KEYCNT at 0x04000130. The UI layer never touches
//! emulation state directly — it only ever sends `press`/`release` events
//! for a button, matching the hardware's own "buttons are just bits" model.

/// Size of the I/O block this module owns: KEYINPUT (2 bytes) + KEYCNT
/// (2 bytes), starting at offset 0x130 relative to 0x04000000.
pub const IO_BASE: usize = 0x130;
pub(crate) const IO_SIZE: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    A,
    B,
    Select,
    Start,
    Right,
    Left,
    Up,
    Down,
    R,
    L,
}

impl Button {
    /// KEYINPUT/KEYCNT bit index — also the stable numeric ID the FFI
    /// bridges (Kotlin/Swift) use, so this ordering is part of the
    /// cross-language contract, not just an internal detail.
    fn bit(self) -> u16 {
        match self {
            Button::A => 0,
            Button::B => 1,
            Button::Select => 2,
            Button::Start => 3,
            Button::Right => 4,
            Button::Left => 5,
            Button::Up => 6,
            Button::Down => 7,
            Button::R => 8,
            Button::L => 9,
        }
    }

    pub fn from_index(index: i32) -> Option<Button> {
        match index {
            0 => Some(Button::A),
            1 => Some(Button::B),
            2 => Some(Button::Select),
            3 => Some(Button::Start),
            4 => Some(Button::Right),
            5 => Some(Button::Left),
            6 => Some(Button::Up),
            7 => Some(Button::Down),
            8 => Some(Button::R),
            9 => Some(Button::L),
            _ => None,
        }
    }
}

/// Real hardware default: all 10 button bits set (released), remaining
/// bits (10-15) unused and read as 1.
const RELEASED: u16 = 0xFFFF;

#[derive(Debug)]
pub struct Joypad {
    /// Active-low: a 0 bit means that button is currently held.
    keyinput: u16,
    keycnt: u16,
}

impl Joypad {
    pub fn new() -> Self {
        Joypad { keyinput: RELEASED, keycnt: 0 }
    }

    pub fn press(&mut self, button: Button) {
        self.keyinput &= !(1 << button.bit());
    }

    pub fn release(&mut self, button: Button) {
        self.keyinput |= 1 << button.bit();
    }

    pub fn is_pressed(&self, button: Button) -> bool {
        self.keyinput & (1 << button.bit()) == 0
    }

    pub(crate) fn read_register(&self, offset: usize) -> u8 {
        match offset {
            0 => (self.keyinput & 0xFF) as u8,
            1 => (self.keyinput >> 8) as u8,
            2 => (self.keycnt & 0xFF) as u8,
            3 => (self.keycnt >> 8) as u8,
            _ => 0,
        }
    }

    pub(crate) fn write_register(&mut self, offset: usize, value: u8) {
        match offset {
            0 | 1 => {} // KEYINPUT is read-only, driven by press()/release()
            2 => self.keycnt = (self.keycnt & 0xFF00) | value as u16,
            3 => self.keycnt = (self.keycnt & 0x00FF) | ((value as u16) << 8),
            _ => {}
        }
    }

    /// Whether the currently held buttons satisfy KEYCNT's IRQ condition
    /// (bits 0-9 select which buttons matter, bit 15 picks OR vs AND).
    /// Computed but not dispatched anywhere yet — actually raising a
    /// joypad interrupt needs the interrupt controller (Etapa 9).
    pub fn irq_condition_met(&self) -> bool {
        let irq_enabled = self.keycnt & 0x4000 != 0;
        if !irq_enabled {
            return false;
        }
        let selected = self.keycnt & 0x03FF;
        if selected == 0 {
            return false;
        }
        let held = (!self.keyinput) & 0x03FF;
        let require_all = self.keycnt & 0x8000 != 0;
        if require_all {
            held & selected == selected
        } else {
            held & selected != 0
        }
    }
}

impl Default for Joypad {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_with_every_button_released() {
        let joypad = Joypad::new();
        for i in 0..10 {
            assert!(!joypad.is_pressed(Button::from_index(i).unwrap()));
        }
        assert_eq!(joypad.read_register(0), 0xFF);
        assert_eq!(joypad.read_register(1), 0xFF);
    }

    #[test]
    fn press_and_release_flip_only_that_bit() {
        let mut joypad = Joypad::new();
        joypad.press(Button::A);
        assert!(joypad.is_pressed(Button::A));
        assert!(!joypad.is_pressed(Button::B));
        assert_eq!(joypad.read_register(0) & 0x01, 0, "A's bit is active-low");

        joypad.press(Button::Start);
        assert!(joypad.is_pressed(Button::A));
        assert!(joypad.is_pressed(Button::Start));

        joypad.release(Button::A);
        assert!(!joypad.is_pressed(Button::A));
        assert!(joypad.is_pressed(Button::Start));
    }

    #[test]
    fn keyinput_write_is_ignored() {
        let mut joypad = Joypad::new();
        joypad.write_register(0, 0x00);
        joypad.write_register(1, 0x00);
        assert_eq!(joypad.read_register(0), 0xFF, "KEYINPUT is read-only");
    }

    #[test]
    fn keycnt_roundtrips() {
        let mut joypad = Joypad::new();
        joypad.write_register(2, 0x3F);
        joypad.write_register(3, 0xC0);
        assert_eq!(joypad.read_register(2), 0x3F);
        assert_eq!(joypad.read_register(3), 0xC0);
    }

    #[test]
    fn irq_condition_or_mode_fires_on_any_selected_button() {
        let mut joypad = Joypad::new();
        joypad.write_register(2, 0b0000_0011); // select A, B
        joypad.write_register(3, 0x40); // IRQ enable (bit14), OR mode (bit15=0)
        assert!(!joypad.irq_condition_met());

        joypad.press(Button::B);
        assert!(joypad.irq_condition_met());
    }

    #[test]
    fn irq_condition_and_mode_requires_every_selected_button() {
        let mut joypad = Joypad::new();
        joypad.write_register(2, 0b0000_0011); // select A, B
        joypad.write_register(3, 0xC0); // IRQ enable + AND mode (bits 14,15)

        joypad.press(Button::A);
        assert!(!joypad.irq_condition_met(), "only A held, AND needs both");

        joypad.press(Button::B);
        assert!(joypad.irq_condition_met());
    }

    #[test]
    fn irq_disabled_never_fires() {
        let mut joypad = Joypad::new();
        joypad.write_register(2, 0xFF);
        joypad.press(Button::A);
        assert!(!joypad.irq_condition_met(), "KEYCNT bit14 (IRQ enable) was never set");
    }
}
