/// CPSR/SPSR bit masks (ARM7TDMI Program Status Register layout).
pub mod psr {
    pub const N: u32 = 1 << 31;
    pub const Z: u32 = 1 << 30;
    pub const C: u32 = 1 << 29;
    pub const V: u32 = 1 << 28;
    pub const I: u32 = 1 << 7; // IRQ disable
    pub const F: u32 = 1 << 6; // FIQ disable
    pub const T: u32 = 1 << 5; // Thumb state
    pub const MODE_MASK: u32 = 0x1F;
}

/// CPU operating mode, encoded in CPSR bits 4:0. Each privileged mode (all
/// but User) banks its own r13/r14 and SPSR; FIQ additionally banks r8-r12.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    User = 0b10000,
    Fiq = 0b10001,
    Irq = 0b10010,
    Supervisor = 0b10011,
    Abort = 0b10111,
    Undefined = 0b11011,
    System = 0b11111,
}

impl Mode {
    pub fn from_bits(bits: u32) -> Option<Mode> {
        match bits & psr::MODE_MASK {
            0b10000 => Some(Mode::User),
            0b10001 => Some(Mode::Fiq),
            0b10010 => Some(Mode::Irq),
            0b10011 => Some(Mode::Supervisor),
            0b10111 => Some(Mode::Abort),
            0b11011 => Some(Mode::Undefined),
            0b11111 => Some(Mode::System),
            _ => None,
        }
    }

    pub fn bits(self) -> u32 {
        self as u32
    }

    /// Whether this mode has its own banked r13/r14/SPSR (every mode but User).
    pub fn is_privileged(self) -> bool {
        !matches!(self, Mode::User)
    }
}

/// Index into the r13/r14/SPSR bank arrays. User and System share a bank —
/// System is "privileged User", introduced so the OS can access user-mode
/// registers from a privileged mode, and the real CPU has no separate
/// banked storage for it.
fn bank_index(mode: Mode) -> usize {
    match mode {
        Mode::User | Mode::System => 0,
        Mode::Fiq => 1,
        Mode::Irq => 2,
        Mode::Supervisor => 3,
        Mode::Abort => 4,
        Mode::Undefined => 5,
    }
}

const BANK_COUNT: usize = 6;

/// The ARM7TDMI register file: r0-r15 as CPU instructions see them, plus
/// the banked copies that mode switches swap in and out, plus CPSR/SPSR.
///
/// r0-r15 are always kept as the *currently visible* set (r13=SP, r14=LR,
/// r15=PC by convention). On a mode change, [`Self::set_mode`] copies the
/// outgoing mode's r13/r14 (and r8-r12 for FIQ) into its bank and loads the
/// incoming mode's bank into r13/r14 — this keeps every other instruction's
/// register access a plain array index, at the cost of doing the swap only
/// on the relatively rare mode transitions.
#[derive(Debug)]
pub struct Registers {
    r: [u32; 16],
    r8_12_fiq: [u32; 5],
    r8_12_other: [u32; 5],
    r13_bank: [u32; BANK_COUNT],
    r14_bank: [u32; BANK_COUNT],
    spsr_bank: [u32; BANK_COUNT],
    cpsr: u32,
    mode: Mode,
}

impl Registers {
    /// Hardware reset state: PC=0, Supervisor mode, IRQ/FIQ disabled, ARM
    /// (not Thumb) instruction set. Stack pointers are left at 0 — on real
    /// hardware those are only ever set by the BIOS's startup code, not by
    /// reset itself.
    pub fn reset() -> Self {
        let mode = Mode::Supervisor;
        Registers {
            r: [0; 16],
            r8_12_fiq: [0; 5],
            r8_12_other: [0; 5],
            r13_bank: [0; BANK_COUNT],
            r14_bank: [0; BANK_COUNT],
            spsr_bank: [0; BANK_COUNT],
            cpsr: mode.bits() | psr::I | psr::F,
            mode,
        }
    }

    pub fn r(&self, index: usize) -> u32 {
        self.r[index]
    }

    pub fn set_r(&mut self, index: usize, value: u32) {
        self.r[index] = value;
    }

    pub fn pc(&self) -> u32 {
        self.r[15]
    }

    pub fn set_pc(&mut self, value: u32) {
        self.r[15] = value;
    }

    pub fn cpsr(&self) -> u32 {
        self.cpsr
    }

    /// Overwrites the whole CPSR (as MSR does), banking registers if the
    /// mode field changed.
    pub fn set_cpsr(&mut self, value: u32) {
        if let Some(new_mode) = Mode::from_bits(value) {
            if new_mode != self.mode {
                self.bank_switch(new_mode);
            }
        }
        self.cpsr = value;
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Changes only the mode field, preserving flags/control bits — what
    /// exception entry/return does, as opposed to a full MSR of CPSR.
    pub fn set_mode(&mut self, new_mode: Mode) {
        if new_mode != self.mode {
            self.bank_switch(new_mode);
        }
        self.cpsr = (self.cpsr & !psr::MODE_MASK) | new_mode.bits();
    }

    fn bank_switch(&mut self, new_mode: Mode) {
        let old = bank_index(self.mode);
        let new = bank_index(new_mode);

        if self.mode == Mode::Fiq {
            self.r8_12_fiq.copy_from_slice(&self.r[8..13]);
        } else {
            self.r8_12_other.copy_from_slice(&self.r[8..13]);
        }
        self.r13_bank[old] = self.r[13];
        self.r14_bank[old] = self.r[14];

        if new_mode == Mode::Fiq {
            self.r[8..13].copy_from_slice(&self.r8_12_fiq);
        } else {
            self.r[8..13].copy_from_slice(&self.r8_12_other);
        }
        self.r[13] = self.r13_bank[new];
        self.r[14] = self.r14_bank[new];

        self.mode = new_mode;
    }

    pub fn flag(&self, bit: u32) -> bool {
        self.cpsr & bit != 0
    }

    pub fn set_flag(&mut self, bit: u32, value: bool) {
        if value {
            self.cpsr |= bit;
        } else {
            self.cpsr &= !bit;
        }
    }

    pub fn in_thumb_state(&self) -> bool {
        self.flag(psr::T)
    }

    /// SPSR of the current mode. Reading this in User/System mode (which
    /// have no SPSR) is undefined on real hardware; this returns the
    /// current CPSR as a harmless fallback rather than panicking.
    pub fn spsr(&self) -> u32 {
        if self.mode.is_privileged() {
            self.spsr_bank[bank_index(self.mode)]
        } else {
            self.cpsr
        }
    }

    pub fn set_spsr(&mut self, value: u32) {
        if self.mode.is_privileged() {
            self.spsr_bank[bank_index(self.mode)] = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_state_matches_hardware_reset() {
        let regs = Registers::reset();
        assert_eq!(regs.pc(), 0);
        assert_eq!(regs.mode(), Mode::Supervisor);
        assert!(regs.flag(psr::I));
        assert!(regs.flag(psr::F));
        assert!(!regs.flag(psr::T));
    }

    #[test]
    fn r13_r14_are_private_per_mode() {
        let mut regs = Registers::reset();
        regs.set_r(13, 0x1000);
        regs.set_r(14, 0x2000);

        regs.set_mode(Mode::Irq);
        regs.set_r(13, 0x3000);
        regs.set_r(14, 0x4000);

        regs.set_mode(Mode::Supervisor);
        assert_eq!(regs.r(13), 0x1000, "SVC bank must be untouched by IRQ writes");
        assert_eq!(regs.r(14), 0x2000);

        regs.set_mode(Mode::Irq);
        assert_eq!(regs.r(13), 0x3000);
        assert_eq!(regs.r(14), 0x4000);
    }

    #[test]
    fn user_and_system_share_the_same_bank() {
        let mut regs = Registers::reset();
        regs.set_mode(Mode::User);
        regs.set_r(13, 0xAAAA);

        regs.set_mode(Mode::System);
        assert_eq!(regs.r(13), 0xAAAA, "System reuses the User bank");
    }

    #[test]
    fn r8_to_r12_are_private_only_for_fiq() {
        let mut regs = Registers::reset();
        regs.set_mode(Mode::User);
        for i in 8..=12 {
            regs.set_r(i, 0x1000 + i as u32);
        }

        regs.set_mode(Mode::Fiq);
        for i in 8..=12 {
            regs.set_r(i, 0x9000 + i as u32);
        }

        regs.set_mode(Mode::Irq); // shares the "other" bank with User
        for i in 8..=12 {
            assert_eq!(regs.r(i), 0x1000 + i as u32, "r{i} should still hold the User-mode value");
        }

        regs.set_mode(Mode::Fiq);
        for i in 8..=12 {
            assert_eq!(regs.r(i), 0x9000 + i as u32, "r{i} should hold the FIQ-private value");
        }
    }

    #[test]
    fn spsr_is_private_per_privileged_mode_and_absent_in_user_mode() {
        let mut regs = Registers::reset();
        regs.set_mode(Mode::Irq);
        regs.set_spsr(0x1111);
        regs.set_mode(Mode::Abort);
        regs.set_spsr(0x2222);

        regs.set_mode(Mode::Irq);
        assert_eq!(regs.spsr(), 0x1111);

        regs.set_mode(Mode::User);
        regs.set_spsr(0xDEAD); // no SPSR in User mode: must be a no-op
        assert_eq!(regs.spsr(), regs.cpsr());
    }

    #[test]
    fn set_cpsr_banks_registers_when_the_mode_field_changes() {
        let mut regs = Registers::reset(); // starts in Supervisor
        regs.set_r(13, 0x1234);

        let irq_cpsr = (regs.cpsr() & !psr::MODE_MASK) | Mode::Irq.bits();
        regs.set_cpsr(irq_cpsr);
        assert_eq!(regs.mode(), Mode::Irq);
        assert_ne!(regs.r(13), 0x1234, "IRQ has its own r13 bank");
    }

    #[test]
    fn flags_round_trip_through_cpsr() {
        let mut regs = Registers::reset();
        regs.set_flag(psr::Z, true);
        regs.set_flag(psr::N, false);
        assert!(regs.flag(psr::Z));
        assert!(!regs.flag(psr::N));
    }
}
