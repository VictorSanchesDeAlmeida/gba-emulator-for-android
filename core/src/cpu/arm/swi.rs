use crate::cpu::{psr, Mode, Registers};

const SWI_VECTOR: u32 = 0x0000_0008;

/// SWI: the standard mechanism GBA games use to call BIOS functions.
/// Saves CPSR to SPSR_svc, saves the return address to LR_svc, switches to
/// Supervisor mode with IRQs disabled and ARM state forced, then jumps to
/// the SWI exception vector. There's no BIOS loaded to actually service the
/// call yet — this only implements the CPU-side exception mechanics.
pub fn execute(regs: &mut Registers, instr_addr: u32) -> (u32, bool) {
    let return_addr = instr_addr.wrapping_add(4);
    let old_cpsr = regs.cpsr();

    regs.set_mode(Mode::Supervisor);
    regs.set_spsr(old_cpsr);
    regs.set_r(14, return_addr);
    regs.set_flag(psr::I, true);
    regs.set_flag(psr::T, false);
    regs.set_pc(SWI_VECTOR);

    (3, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swi_enters_supervisor_mode_and_saves_state() {
        let mut regs = Registers::reset();
        regs.set_mode(Mode::User);
        regs.set_flag(psr::T, false);

        let before_cpsr = regs.cpsr();
        execute(&mut regs, 0x0800_0100);

        assert_eq!(regs.mode(), Mode::Supervisor);
        assert_eq!(regs.r(14), 0x0800_0104);
        assert_eq!(regs.pc(), SWI_VECTOR);
        assert!(regs.flag(psr::I));
        assert!(!regs.flag(psr::T));

        regs.set_mode(Mode::User); // bounce through User just to read spsr_svc back
        regs.set_mode(Mode::Supervisor);
        assert_eq!(regs.spsr(), before_cpsr);
    }
}
