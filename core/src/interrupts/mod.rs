//! The GBA interrupt controller: IE (enable), IF (request/acknowledge) and
//! IME (master enable) at 0x04000200/0x04000202/0x04000208. Peripherals
//! (PPU, Timers, Joypad, ...) call [`Interrupts::request`] when their own
//! condition fires *and* their own IRQ-enable bit is set (e.g. DISPSTAT's
//! VBlank-IRQ-enable, TMxCNT_H's IRQ-enable) — this controller only decides
//! whether that request actually reaches the CPU.

/// I/O block for IE (0x200-0x201) + IF (0x202-0x203).
pub const IE_IF_BASE: usize = 0x200;
pub(crate) const IE_IF_SIZE: usize = 4;

/// I/O block for IME. Real hardware only uses bit 0 of the low byte; the
/// rest of the 4 bytes are unused padding, stored but inert.
pub const IME_BASE: usize = 0x208;
pub(crate) const IME_SIZE: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptSource {
    VBlank,
    HBlank,
    VCounter,
    Timer0,
    Timer1,
    Timer2,
    Timer3,
    Serial,
    Dma0,
    Dma1,
    Dma2,
    Dma3,
    Keypad,
    GamePak,
}

impl InterruptSource {
    /// IE/IF bit index — fixed by hardware, not an internal choice.
    fn bit(self) -> u16 {
        match self {
            InterruptSource::VBlank => 0,
            InterruptSource::HBlank => 1,
            InterruptSource::VCounter => 2,
            InterruptSource::Timer0 => 3,
            InterruptSource::Timer1 => 4,
            InterruptSource::Timer2 => 5,
            InterruptSource::Timer3 => 6,
            InterruptSource::Serial => 7,
            InterruptSource::Dma0 => 8,
            InterruptSource::Dma1 => 9,
            InterruptSource::Dma2 => 10,
            InterruptSource::Dma3 => 11,
            InterruptSource::Keypad => 12,
            InterruptSource::GamePak => 13,
        }
    }

    pub fn from_timer_index(index: usize) -> InterruptSource {
        match index {
            0 => InterruptSource::Timer0,
            1 => InterruptSource::Timer1,
            2 => InterruptSource::Timer2,
            3 => InterruptSource::Timer3,
            _ => panic!("timer index out of range: {index}"),
        }
    }

    pub fn from_dma_index(index: usize) -> InterruptSource {
        match index {
            0 => InterruptSource::Dma0,
            1 => InterruptSource::Dma1,
            2 => InterruptSource::Dma2,
            3 => InterruptSource::Dma3,
            _ => panic!("DMA index out of range: {index}"),
        }
    }
}

#[derive(Debug)]
pub struct Interrupts {
    ie: u16,
    iflag: u16,
    ime: bool,
}

impl Interrupts {
    pub fn new() -> Self {
        Interrupts { ie: 0, iflag: 0, ime: false }
    }

    /// Sets a source's IF bit. Idempotent — safe to call every tick while a
    /// level-held condition (like a timer overflow this instant) is true;
    /// it never re-sets a bit the CPU already acknowledged past this call.
    pub fn request(&mut self, source: InterruptSource) {
        self.iflag |= 1 << source.bit();
    }

    /// Whether the CPU should take an IRQ exception right now: master
    /// enable on, and at least one source is both enabled (IE) and pending
    /// (IF). Does *not* check CPSR's I bit — that's the CPU's own gate.
    pub fn pending(&self) -> bool {
        self.ime && (self.ie & self.iflag != 0)
    }

    /// The bits currently both enabled (IE) and requested (IF) — what a
    /// real interrupt handler would acknowledge in one pass. Used by
    /// [`crate::bios`]'s synthetic IRQ servicing, which needs this value
    /// independent of `pending()`'s IME gate (the bits still get
    /// acknowledged as part of servicing even though IME is what decided
    /// whether servicing happens at all).
    pub(crate) fn ie_and_iflag(&self) -> u16 {
        self.ie & self.iflag
    }

    /// Write-1-to-clear acknowledgement of specific IF bits, the same
    /// semantics as a real IF register write — used by [`crate::bios`]'s
    /// synthetic interrupt servicing instead of going through the I/O
    /// byte-write path.
    pub(crate) fn acknowledge(&mut self, mask: u16) {
        self.iflag &= !mask;
    }

    pub(crate) fn read_ie_if(&self, offset: usize) -> u8 {
        match offset {
            0 => (self.ie & 0xFF) as u8,
            1 => (self.ie >> 8) as u8,
            2 => (self.iflag & 0xFF) as u8,
            3 => (self.iflag >> 8) as u8,
            _ => 0,
        }
    }

    pub(crate) fn write_ie_if(&mut self, offset: usize, value: u8) {
        match offset {
            0 => self.ie = (self.ie & 0xFF00) | value as u16,
            1 => self.ie = (self.ie & 0x00FF) | ((value as u16) << 8),
            // IF is write-1-to-clear: a written 1 bit clears that IF bit, a
            // written 0 bit leaves it alone. Inverting a zero-extended u8
            // already turns the untouched half into all-1s (a no-op mask),
            // so this single AND handles both halves correctly.
            2 => self.iflag &= !(value as u16),
            3 => self.iflag &= !((value as u16) << 8),
            _ => {}
        }
    }

    pub(crate) fn read_ime(&self, offset: usize) -> u8 {
        if offset == 0 {
            self.ime as u8
        } else {
            0
        }
    }

    pub(crate) fn write_ime(&mut self, offset: usize, value: u8) {
        if offset == 0 {
            self.ime = value & 1 != 0;
        }
    }
}

impl Default for Interrupts {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ie_and_iflag_ignores_ime() {
        let mut irq = Interrupts::new();
        irq.write_ie_if(0, 0x01); // IE bit0
        irq.request(InterruptSource::VBlank);
        assert_eq!(irq.ie_and_iflag(), 0x01, "IME being off must not affect this");
    }

    #[test]
    fn acknowledge_clears_only_the_given_bits() {
        let mut irq = Interrupts::new();
        irq.request(InterruptSource::VBlank);
        irq.request(InterruptSource::Timer0);
        irq.acknowledge(0x01);
        assert_eq!(irq.read_ie_if(2), 0b0000_1000, "only VBlank's bit was acknowledged");
    }

    #[test]
    fn no_interrupt_pending_by_default() {
        let irq = Interrupts::new();
        assert!(!irq.pending());
    }

    #[test]
    fn request_alone_does_not_make_it_pending() {
        let mut irq = Interrupts::new();
        irq.request(InterruptSource::VBlank);
        assert!(!irq.pending(), "IE and IME are still off");
    }

    #[test]
    fn pending_requires_ie_and_ime_and_iflag() {
        let mut irq = Interrupts::new();
        irq.write_ie_if(0, 0x01); // IE bit0 (VBlank)
        irq.request(InterruptSource::VBlank);
        assert!(!irq.pending(), "IME still off");

        irq.write_ime(0, 1);
        assert!(irq.pending());
    }

    #[test]
    fn if_write_one_clears_only_that_bit() {
        let mut irq = Interrupts::new();
        irq.request(InterruptSource::VBlank);
        irq.request(InterruptSource::Timer0);
        assert_eq!(irq.read_ie_if(2), 0b0000_1001);

        irq.write_ie_if(2, 0b0000_0001); // acknowledge VBlank only
        assert_eq!(irq.read_ie_if(2), 0b0000_1000, "Timer0's IF bit must survive");
    }

    #[test]
    fn if_write_zero_bit_leaves_it_set() {
        let mut irq = Interrupts::new();
        irq.request(InterruptSource::VBlank);
        irq.write_ie_if(2, 0x00);
        assert_eq!(irq.read_ie_if(2) & 0x01, 0x01, "writing 0 must not clear");
    }

    #[test]
    fn ie_roundtrips_both_bytes() {
        let mut irq = Interrupts::new();
        irq.write_ie_if(0, 0xAB);
        irq.write_ie_if(1, 0xCD);
        assert_eq!(irq.read_ie_if(0), 0xAB);
        assert_eq!(irq.read_ie_if(1), 0xCD);
    }

    #[test]
    fn ime_only_uses_bit_zero() {
        let mut irq = Interrupts::new();
        irq.write_ime(0, 0xFE); // bit0 clear, rest set
        assert_eq!(irq.read_ime(0), 0);
        irq.write_ime(0, 0x01);
        assert_eq!(irq.read_ime(0), 1);
    }

    #[test]
    fn acknowledging_the_only_pending_source_clears_pending() {
        let mut irq = Interrupts::new();
        irq.write_ie_if(0, 0x01);
        irq.write_ime(0, 1);
        irq.request(InterruptSource::VBlank);
        assert!(irq.pending());

        irq.write_ie_if(2, 0x01); // acknowledge
        assert!(!irq.pending());
    }
}
