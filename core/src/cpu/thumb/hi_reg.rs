use crate::cpu::arm::alu;
use crate::cpu::{psr, Registers};

/// Format 5 — Hi register operations / branch exchange: `ADD/CMP/MOV`
/// between any of r0-r15 (via the H1/H2 bits selecting the r8-r15 half),
/// and `BX`. This is the only Format besides BL that can touch the PC
/// directly from Thumb state, so it's kept separate from Format 4's
/// low-register-only ALU ops.
///
/// Returns `(cycles, pc_written)`.
pub fn execute(regs: &mut Registers, opcode: u16, instr_addr: u32) -> (u32, bool) {
    let op = (opcode >> 8) & 0b11;
    let h1 = (opcode >> 7) & 1 == 1;
    let h2 = (opcode >> 6) & 1 == 1;
    let rs = (((opcode >> 3) & 0x7) as usize) + if h2 { 8 } else { 0 };
    let rd = ((opcode & 0x7) as usize) + if h1 { 8 } else { 0 };

    // Thumb's own PC read-ahead: instruction address + 4 (half of ARM's +8,
    // matching the 2-byte instruction size and the same 2-stage-ahead
    // pipeline convention).
    let rs_value = if rs == 15 { instr_addr.wrapping_add(4) } else { regs.r(rs) };

    match op {
        0b00 => {
            // ADD Rd, Rs — flags unaffected, matching the Thumb quick reference.
            let rd_value = if rd == 15 { instr_addr.wrapping_add(4) } else { regs.r(rd) };
            let result = rd_value.wrapping_add(rs_value);
            regs.set_r(rd, result);
            (if rd == 15 { 3 } else { 1 }, rd == 15)
        }
        0b01 => {
            // CMP Rd, Rs — sets flags like ARM CMP, never writes Rd.
            let rd_value = if rd == 15 { instr_addr.wrapping_add(4) } else { regs.r(rd) };
            let r = alu::sub_with_carry(rd_value, rs_value, true);
            regs.set_flag(psr::Z, r.value == 0);
            regs.set_flag(psr::N, (r.value >> 31) & 1 == 1);
            regs.set_flag(psr::C, r.carry);
            regs.set_flag(psr::V, r.overflow);
            (1, false)
        }
        0b10 => {
            // MOV Rd, Rs — a plain copy, no flags touched at all.
            regs.set_r(rd, rs_value);
            (if rd == 15 { 3 } else { 1 }, rd == 15)
        }
        0b11 => {
            // BX Rs
            let thumb = rs_value & 1 == 1;
            regs.set_flag(psr::T, thumb);
            regs.set_pc(rs_value & if thumb { !1 } else { !3 });
            (3, true)
        }
        _ => unreachable!("2-bit field"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(op: u32, h1: bool, h2: bool, rs_low: u32, rd_low: u32) -> u16 {
        ((0b010001 << 10) | (op << 8) | ((h1 as u32) << 7) | ((h2 as u32) << 6) | (rs_low << 3) | rd_low) as u16
    }

    #[test]
    fn add_hi_register_no_flags() {
        let mut regs = Registers::reset();
        regs.set_flag(psr::Z, true);
        regs.set_r(9, 100); // r9 = hi register (rs_low=1, h2=1 -> r9)
        regs.set_r(0, 5);
        let op = encode(0b00, false, true, 1, 0); // ADD r0, r9
        execute(&mut regs, op, 0);
        assert_eq!(regs.r(0), 105);
        assert!(regs.flag(psr::Z), "ADD in Format 5 must not touch flags");
    }

    #[test]
    fn mov_to_pc_branches_without_flags() {
        let mut regs = Registers::reset();
        regs.set_r(1, 0x0800_2000);
        let op = encode(0b10, true, false, 1, 7); // MOV r15 (h1=1,rd_low=7), r1
        let (_, pc_written) = execute(&mut regs, op, 0);
        assert!(pc_written);
        assert_eq!(regs.pc(), 0x0800_2000);
    }

    #[test]
    fn bx_switches_to_arm_state_on_even_target() {
        let mut regs = Registers::reset();
        regs.set_flag(psr::T, true);
        regs.set_r(0, 0x0800_3000);
        let op = encode(0b11, false, false, 0, 0); // BX r0
        let (_, pc_written) = execute(&mut regs, op, 0);
        assert!(pc_written);
        assert!(!regs.in_thumb_state());
        assert_eq!(regs.pc(), 0x0800_3000);
    }

    #[test]
    fn cmp_reads_pc_as_instruction_address_plus_4() {
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0000_1004); // will compare against PC+4 when Rd=15... use Rs=15 instead
        let op = encode(0b01, false, true, 7, 0); // CMP r0, r15 (rs_low=7,h2=1 -> r15)
        execute(&mut regs, op, 0x0000_1000);
        assert!(regs.flag(psr::Z), "r0 == instr_addr(0x1000)+4");
    }
}
