use crate::cpu::Registers;

/// Executes B / BL. The 24-bit signed offset is a word count relative to
/// the instruction's own address + 8 (the ARM PC-read-ahead convention),
/// so it's sign-extended then shifted left 2 before adding.
pub fn execute(regs: &mut Registers, opcode: u32, instr_addr: u32) -> (u32, bool) {
    let link = (opcode >> 24) & 1 == 1;
    let offset_raw = opcode & 0x00FF_FFFF;
    let signed_words = ((offset_raw << 8) as i32) >> 8;
    let byte_offset = signed_words << 2;

    let target = instr_addr.wrapping_add(8).wrapping_add(byte_offset as u32);

    if link {
        regs.set_r(14, instr_addr.wrapping_add(4));
    }
    regs.set_pc(target);

    (3, true) // pipeline flush; refined once Etapa 5 models timing precisely
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_branch(link: bool, offset_words: i32) -> u32 {
        let cond = 0xE << 28;
        let group = 0b101 << 25;
        let l = (link as u32) << 24;
        let offset = (offset_words as u32) & 0x00FF_FFFF;
        cond | group | l | offset
    }

    #[test]
    fn forward_branch_targets_pc_plus_8_plus_offset() {
        let mut regs = Registers::reset();
        let op = encode_branch(false, 4); // +4 words = +16 bytes
        let (cycles, pc_written) = execute(&mut regs, op, 0x0000_1000);
        assert_eq!(regs.pc(), 0x0000_1000 + 8 + 16);
        assert!(pc_written);
        assert_eq!(cycles, 3);
    }

    #[test]
    fn backward_branch_sign_extends_correctly() {
        let mut regs = Registers::reset();
        let op = encode_branch(false, -2); // -2 words = -8 bytes
        execute(&mut regs, op, 0x0000_2000);
        assert_eq!(regs.pc(), 0x0000_2000 + 8 - 8);
    }

    #[test]
    fn bl_sets_lr_to_the_next_instruction_and_branches() {
        let mut regs = Registers::reset();
        let op = encode_branch(true, 0);
        execute(&mut regs, op, 0x0000_3000);
        assert_eq!(regs.r(14), 0x0000_3004, "LR = address of the instruction after BL");
        assert_eq!(regs.pc(), 0x0000_3000 + 8);
    }

    #[test]
    fn plain_b_does_not_touch_lr() {
        let mut regs = Registers::reset();
        regs.set_r(14, 0xCAFE_BABE);
        let op = encode_branch(false, 1);
        execute(&mut regs, op, 0);
        assert_eq!(regs.r(14), 0xCAFE_BABE);
    }
}
