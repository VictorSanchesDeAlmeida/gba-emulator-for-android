use crate::cpu::{psr, Registers};

/// BX: jumps to `Rm & !1`, switching to Thumb state if `Rm`'s bit 0 is set.
/// This is the only way the CPU ever enters Thumb state.
pub fn execute(regs: &mut Registers, opcode: u32, instr_addr: u32) -> (u32, bool) {
    let rm = (opcode & 0xF) as usize;
    let target = if rm == 15 { instr_addr.wrapping_add(8) } else { regs.r(rm) };

    let thumb = target & 1 == 1;
    regs.set_flag(psr::T, thumb);
    regs.set_pc(target & if thumb { !1 } else { !3 });

    (3, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_bx(cond: u32, rm: u32) -> u32 {
        (cond << 28) | 0x012F_FF10 | rm
    }

    #[test]
    fn bx_to_odd_address_enters_thumb_state() {
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0800_1001);
        execute(&mut regs, encode_bx(0xE, 0), 0);
        assert!(regs.in_thumb_state());
        assert_eq!(regs.pc(), 0x0800_1000);
    }

    #[test]
    fn bx_to_even_address_stays_in_arm_state() {
        let mut regs = Registers::reset();
        regs.set_flag(psr::T, true);
        regs.set_r(0, 0x0800_2004);
        execute(&mut regs, encode_bx(0xE, 0), 0);
        assert!(!regs.in_thumb_state());
        assert_eq!(regs.pc(), 0x0800_2004);
    }
}
