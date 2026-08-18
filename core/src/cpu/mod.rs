mod arm;
mod registers;
mod thumb;

pub use registers::{psr, Mode, Registers};

use crate::memory::Memory;

const IRQ_VECTOR: u32 = 0x0000_0018;

/// An instruction the CPU decoded correctly but doesn't execute yet.
/// Etapa 4 covers ARM Data Processing and Branch/Branch-Link end to end;
/// every other ARM group (Multiply, Single/Block Data Transfer, PSR
/// Transfer, Branch-and-Exchange, Software Interrupt) and the entire Thumb
/// instruction set surface this instead of executing silently-wrong
/// behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuError {
    UnimplementedArm { opcode: u32, pc: u32 },
    UnimplementedThumb { opcode: u16, pc: u32 },
}

/// The ARM7TDMI core: register file plus the fetch/decode/execute step.
/// Talks to memory only through the [`Memory`] trait — it never touches
/// `Bus`, `Cartridge`, or any other component directly.
#[derive(Debug)]
pub struct Cpu {
    pub registers: Registers,
}

impl Cpu {
    pub fn new() -> Self {
        Cpu { registers: Registers::reset() }
    }

    /// Fetches and executes one instruction, advancing PC unless the
    /// instruction itself already redirected it (a branch, or any
    /// instruction that writes r15 as its destination).
    pub fn step<M: Memory>(&mut self, mem: &mut M) -> Result<u32, CpuError> {
        if self.registers.in_thumb_state() {
            let pc = self.registers.pc() & !1;
            let opcode = mem.read16(pc);
            let (cycles, pc_written) = thumb::execute(&mut self.registers, mem, opcode, pc)?;
            if !pc_written {
                self.registers.set_pc(pc.wrapping_add(2));
            }
            Ok(cycles)
        } else {
            let pc = self.registers.pc() & !3;
            let opcode = mem.read32(pc);
            let (cycles, pc_written) = arm::execute(&mut self.registers, mem, opcode, pc)?;
            if !pc_written {
                self.registers.set_pc(pc.wrapping_add(4));
            }
            Ok(cycles)
        }
    }

    /// Takes a pending IRQ instead of executing the next instruction: saves
    /// CPSR to SPSR_irq, sets LR_irq to the not-yet-executed instruction's
    /// address + 4 (the fixed offset the BIOS's `SUBS PC,LR,#4` return
    /// convention expects — the same constant for both ARM and Thumb state,
    /// unlike SWI/UND which differ by state), switches to IRQ mode with
    /// IRQ disabled and ARM state forced, and jumps to the IRQ vector.
    /// Callers must check `!registers.flag(psr::I)` themselves first — this
    /// always takes the exception unconditionally once called.
    pub fn enter_irq(&mut self) {
        let return_addr = self.registers.pc().wrapping_add(4);
        let old_cpsr = self.registers.cpsr();

        self.registers.set_mode(Mode::Irq);
        self.registers.set_spsr(old_cpsr);
        self.registers.set_r(14, return_addr);
        self.registers.set_flag(psr::I, true);
        self.registers.set_flag(psr::T, false);
        self.registers.set_pc(IRQ_VECTOR);
    }
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_irq_saves_state_and_jumps_to_the_irq_vector() {
        let mut cpu = Cpu::new();
        cpu.registers.set_mode(Mode::User);
        cpu.registers.set_flag(psr::T, false);
        cpu.registers.set_pc(0x0800_0100);

        let before_cpsr = cpu.registers.cpsr();
        cpu.enter_irq();

        assert_eq!(cpu.registers.mode(), Mode::Irq);
        assert_eq!(cpu.registers.r(14), 0x0800_0104, "LR_irq = interrupted PC + 4");
        assert_eq!(cpu.registers.pc(), IRQ_VECTOR);
        assert!(cpu.registers.flag(psr::I), "IRQ must be disabled on entry");
        assert!(!cpu.registers.flag(psr::T), "must force ARM state");
        assert_eq!(cpu.registers.spsr(), before_cpsr);
    }

    #[test]
    fn enter_irq_uses_the_same_plus_four_offset_from_thumb_state() {
        let mut cpu = Cpu::new();
        cpu.registers.set_mode(Mode::User);
        cpu.registers.set_flag(psr::T, true);
        cpu.registers.set_pc(0x0800_0200);

        cpu.enter_irq();

        assert_eq!(cpu.registers.r(14), 0x0800_0204, "GBA IRQ entry uses +4 regardless of interrupted state");
    }
}
