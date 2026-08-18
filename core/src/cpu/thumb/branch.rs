use crate::cpu::arm::check_condition;
use crate::cpu::{psr, Mode, Registers};

const SWI_VECTOR: u32 = 0x0000_0008;

/// Format 16 — Conditional branch: `B<cond> label`. Returns `(cycles,
/// pc_written)`; when the condition fails, the branch is skipped (fetched
/// but not taken), same convention as ARM.
pub fn conditional_branch(regs: &Registers, opcode: u16, instr_addr: u32) -> (u32, bool, Option<u32>) {
    let cond = ((opcode >> 8) & 0xF) as u32;
    if !check_condition(cond, regs) {
        return (1, false, None);
    }
    let soffset8 = (opcode & 0xFF) as u8 as i8 as i32;
    let target = instr_addr.wrapping_add(4).wrapping_add((soffset8 * 2) as u32);
    (3, true, Some(target))
}

/// Format 17 — Software interrupt. Same exception mechanics as ARM's SWI
/// (save CPSR to SPSR_svc, LR_svc = return address, jump to the SWI
/// vector, force ARM state for the handler) but the return address is
/// instr_addr+2, matching Thumb's 2-byte instruction size.
pub fn software_interrupt(regs: &mut Registers, instr_addr: u32) -> (u32, bool) {
    let return_addr = instr_addr.wrapping_add(2);
    let old_cpsr = regs.cpsr();

    regs.set_mode(Mode::Supervisor);
    regs.set_spsr(old_cpsr);
    regs.set_r(14, return_addr);
    regs.set_flag(psr::I, true);
    regs.set_flag(psr::T, false);
    regs.set_pc(SWI_VECTOR);

    (3, true)
}

/// Format 18 — Unconditional branch: `B label`.
pub fn unconditional_branch(opcode: u16, instr_addr: u32) -> u32 {
    let offset11 = (opcode & 0x7FF) as u32;
    let signed = ((offset11 << 21) as i32) >> 21; // sign-extend 11 bits
    instr_addr.wrapping_add(4).wrapping_add((signed * 2) as u32)
}

/// Format 19 — Long branch with link, first half (H=0): stashes a partial
/// target in LR. `BL`'s target only becomes known once the second half
/// (bottom 11 bits) arrives, so this half never touches PC.
pub fn branch_link_high(regs: &mut Registers, opcode: u16, instr_addr: u32) {
    let offset11 = (opcode & 0x7FF) as u32;
    let signed = ((offset11 << 21) as i32) >> 21; // sign-extend 11 bits
    let base = instr_addr.wrapping_add(4);
    regs.set_r(14, base.wrapping_add((signed << 12) as u32));
}

/// Format 19 — Long branch with link, second half (H=1): completes the
/// jump using LR as scratch, and sets LR to the return address (with bit 0
/// set, so a later `BX LR` returns into Thumb state).
pub fn branch_link_low(regs: &mut Registers, opcode: u16, instr_addr: u32) {
    let offset11 = (opcode & 0x7FF) as u32;
    let return_addr = instr_addr.wrapping_add(2) | 1;
    let target = regs.r(14).wrapping_add(offset11 << 1);
    regs.set_r(14, return_addr);
    regs.set_pc(target);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditional_branch_taken_when_flag_matches() {
        let mut regs = Registers::reset();
        regs.set_flag(psr::Z, true);
        let op = (0b1101 << 12) | (0x0 << 8) | 4u16; // BEQ, offset=4 words->8 bytes
        let (_, pc_written, target) = conditional_branch(&regs, op, 0x0000_1000);
        assert!(pc_written);
        assert_eq!(target, Some(0x0000_1000 + 4 + 8));
    }

    #[test]
    fn conditional_branch_skipped_when_flag_does_not_match() {
        let regs = Registers::reset(); // Z=0
        let op = (0b1101 << 12) | (0x0 << 8) | 4u16; // BEQ
        let (_, pc_written, target) = conditional_branch(&regs, op, 0);
        assert!(!pc_written);
        assert_eq!(target, None);
    }

    #[test]
    fn unconditional_branch_sign_extends_11_bits() {
        // offset11 = -2 (0x7FE in 11-bit two's complement)
        let op = (0b11100 << 11) | 0x7FEu16;
        let target = unconditional_branch(op, 0x0000_2000);
        assert_eq!(target, 0x0000_2000 + 4 - 4);
    }

    #[test]
    fn bl_pair_computes_a_far_target_and_sets_lr_with_thumb_bit() {
        let mut regs = Registers::reset();
        // High half: offset11=1 -> signed<<12 = 0x1000
        let high_op = (0b11110 << 11) | 1u16;
        branch_link_high(&mut regs, high_op, 0x0000_1000);
        assert_eq!(regs.r(14), 0x0000_1000 + 4 + 0x1000);

        // Low half: offset11=2 -> +4 bytes
        let low_op = (0b11111 << 11) | 2u16;
        branch_link_low(&mut regs, low_op, 0x0000_1002);

        assert_eq!(regs.pc(), 0x0000_1000 + 4 + 0x1000 + 4);
        assert_eq!(regs.r(14), (0x0000_1002 + 2) | 1);
    }

    #[test]
    fn swi_enters_supervisor_with_thumb_return_offset() {
        let mut regs = Registers::reset();
        regs.set_mode(Mode::User);
        software_interrupt(&mut regs, 0x0000_2000);
        assert_eq!(regs.mode(), Mode::Supervisor);
        assert_eq!(regs.r(14), 0x0000_2002);
        assert_eq!(regs.pc(), SWI_VECTOR);
        assert!(!regs.in_thumb_state());
    }
}
