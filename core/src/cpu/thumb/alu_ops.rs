use crate::cpu::arm::alu;
use crate::cpu::arm::shifter::{self, ShiftType};
use crate::cpu::{psr, Registers};

fn set_nz(regs: &mut Registers, result: u32) {
    regs.set_flag(psr::Z, result == 0);
    regs.set_flag(psr::N, (result >> 31) & 1 == 1);
}

/// Format 1 — Move shifted register: `LSL/LSR/ASR Rd, Rs, #Offset5`.
pub fn move_shifted_register(regs: &mut Registers, opcode: u16) {
    let op = (opcode >> 11) & 0b11;
    let offset5 = ((opcode >> 6) & 0x1F) as u32;
    let rs = ((opcode >> 3) & 0x7) as usize;
    let rd = (opcode & 0x7) as usize;

    let shift_type = match op {
        0 => ShiftType::Lsl,
        1 => ShiftType::Lsr,
        2 => ShiftType::Asr,
        _ => unreachable!("op=11 belongs to Format 2, excluded by the top-level dispatcher"),
    };

    let carry_in = regs.flag(psr::C);
    let r = shifter::shift(shift_type, regs.r(rs), offset5, carry_in, true);
    regs.set_r(rd, r.value);
    set_nz(regs, r.value);
    regs.set_flag(psr::C, r.carry_out);
}

/// Format 2 — Add/subtract: `ADD/SUB Rd, Rs, Rn` or `ADD/SUB Rd, Rs, #Offset3`.
pub fn add_subtract(regs: &mut Registers, opcode: u16) {
    let immediate = (opcode >> 10) & 1 == 1;
    let subtract = (opcode >> 9) & 1 == 1;
    let operand = ((opcode >> 6) & 0x7) as u32;
    let rs = ((opcode >> 3) & 0x7) as usize;
    let rd = (opcode & 0x7) as usize;

    let rhs = if immediate { operand } else { regs.r(operand as usize) };
    let rs_value = regs.r(rs);

    let r = if subtract {
        alu::sub_with_carry(rs_value, rhs, true)
    } else {
        alu::add_with_carry(rs_value, rhs, false)
    };

    regs.set_r(rd, r.value);
    set_nz(regs, r.value);
    regs.set_flag(psr::C, r.carry);
    regs.set_flag(psr::V, r.overflow);
}

/// Format 3 — Move/compare/add/subtract immediate: `MOV/CMP/ADD/SUB Rd, #Offset8`.
pub fn immediate_op(regs: &mut Registers, opcode: u16) {
    let op = (opcode >> 11) & 0b11;
    let rd = ((opcode >> 8) & 0x7) as usize;
    let imm8 = (opcode & 0xFF) as u32;
    let rd_value = regs.r(rd);

    match op {
        0b00 => {
            // MOV: only N,Z affected — no arithmetic, so C/V are left alone.
            regs.set_r(rd, imm8);
            set_nz(regs, imm8);
        }
        0b01 => {
            // CMP: compare only, doesn't write Rd.
            let r = alu::sub_with_carry(rd_value, imm8, true);
            set_nz(regs, r.value);
            regs.set_flag(psr::C, r.carry);
            regs.set_flag(psr::V, r.overflow);
        }
        0b10 => {
            let r = alu::add_with_carry(rd_value, imm8, false);
            regs.set_r(rd, r.value);
            set_nz(regs, r.value);
            regs.set_flag(psr::C, r.carry);
            regs.set_flag(psr::V, r.overflow);
        }
        0b11 => {
            let r = alu::sub_with_carry(rd_value, imm8, true);
            regs.set_r(rd, r.value);
            set_nz(regs, r.value);
            regs.set_flag(psr::C, r.carry);
            regs.set_flag(psr::V, r.overflow);
        }
        _ => unreachable!("2-bit field"),
    }
}

/// Format 4 — ALU operations: `<OP> Rd, Rs` (Rd is both source and dest).
pub fn alu_operation(regs: &mut Registers, opcode: u16) {
    let op = (opcode >> 6) & 0xF;
    let rs = ((opcode >> 3) & 0x7) as usize;
    let rd = (opcode & 0x7) as usize;
    let rd_value = regs.r(rd);
    let rs_value = regs.r(rs);

    match op {
        0x0 => { let v = rd_value & rs_value; regs.set_r(rd, v); set_nz(regs, v); } // AND
        0x1 => { let v = rd_value ^ rs_value; regs.set_r(rd, v); set_nz(regs, v); } // EOR
        0x2 => { // LSL (register-specified shift amount)
            let amount = rs_value & 0xFF;
            let r = shifter::shift(ShiftType::Lsl, rd_value, amount, regs.flag(psr::C), false);
            regs.set_r(rd, r.value); set_nz(regs, r.value); regs.set_flag(psr::C, r.carry_out);
        }
        0x3 => { // LSR
            let amount = rs_value & 0xFF;
            let r = shifter::shift(ShiftType::Lsr, rd_value, amount, regs.flag(psr::C), false);
            regs.set_r(rd, r.value); set_nz(regs, r.value); regs.set_flag(psr::C, r.carry_out);
        }
        0x4 => { // ASR
            let amount = rs_value & 0xFF;
            let r = shifter::shift(ShiftType::Asr, rd_value, amount, regs.flag(psr::C), false);
            regs.set_r(rd, r.value); set_nz(regs, r.value); regs.set_flag(psr::C, r.carry_out);
        }
        0x5 => { // ADC
            let r = alu::add_with_carry(rd_value, rs_value, regs.flag(psr::C));
            regs.set_r(rd, r.value); set_nz(regs, r.value); regs.set_flag(psr::C, r.carry); regs.set_flag(psr::V, r.overflow);
        }
        0x6 => { // SBC
            let r = alu::sub_with_carry(rd_value, rs_value, regs.flag(psr::C));
            regs.set_r(rd, r.value); set_nz(regs, r.value); regs.set_flag(psr::C, r.carry); regs.set_flag(psr::V, r.overflow);
        }
        0x7 => { // ROR (register-specified)
            let amount = rs_value & 0xFF;
            let r = shifter::shift(ShiftType::Ror, rd_value, amount, regs.flag(psr::C), false);
            regs.set_r(rd, r.value); set_nz(regs, r.value); regs.set_flag(psr::C, r.carry_out);
        }
        0x8 => { let v = rd_value & rs_value; set_nz(regs, v); } // TST
        0x9 => { // NEG: Rd = 0 - Rs
            let r = alu::sub_with_carry(0, rs_value, true);
            regs.set_r(rd, r.value); set_nz(regs, r.value); regs.set_flag(psr::C, r.carry); regs.set_flag(psr::V, r.overflow);
        }
        0xA => { // CMP
            let r = alu::sub_with_carry(rd_value, rs_value, true);
            set_nz(regs, r.value); regs.set_flag(psr::C, r.carry); regs.set_flag(psr::V, r.overflow);
        }
        0xB => { // CMN
            let r = alu::add_with_carry(rd_value, rs_value, false);
            set_nz(regs, r.value); regs.set_flag(psr::C, r.carry); regs.set_flag(psr::V, r.overflow);
        }
        0xC => { let v = rd_value | rs_value; regs.set_r(rd, v); set_nz(regs, v); } // ORR
        0xD => { // MUL
            let v = rd_value.wrapping_mul(rs_value);
            regs.set_r(rd, v); set_nz(regs, v); // C meaningless on ARMv4, left unchanged
        }
        0xE => { let v = rd_value & !rs_value; regs.set_r(rd, v); set_nz(regs, v); } // BIC
        0xF => { let v = !rs_value; regs.set_r(rd, v); set_nz(regs, v); } // MVN
        _ => unreachable!("4-bit field"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsl_immediate_shifts_and_sets_carry() {
        let mut regs = Registers::reset();
        regs.set_r(1, 0x8000_0000);
        // LSL r0, r1, #1
        let op = (0b000 << 13) | (0b00 << 11) | (1 << 6) | (1 << 3) | 0;
        move_shifted_register(&mut regs, op);
        assert_eq!(regs.r(0), 0);
        assert!(regs.flag(psr::C));
        assert!(regs.flag(psr::Z));
    }

    #[test]
    fn add_register_form() {
        let mut regs = Registers::reset();
        regs.set_r(1, 5);
        regs.set_r(2, 3);
        // ADD r0, r1, r2 (Format2, immediate=0, subtract=0)
        let op = (0b000_11 << 11) | (0 << 10) | (0 << 9) | (2 << 6) | (1 << 3) | 0;
        add_subtract(&mut regs, op);
        assert_eq!(regs.r(0), 8);
    }

    #[test]
    fn sub_immediate3_form() {
        let mut regs = Registers::reset();
        regs.set_r(1, 5);
        // SUB r0, r1, #2 (Format2, immediate=1, subtract=1)
        let op = (0b000_11 << 11) | (1 << 10) | (1 << 9) | (2 << 6) | (1 << 3) | 0;
        add_subtract(&mut regs, op);
        assert_eq!(regs.r(0), 3);
    }

    #[test]
    fn mov_immediate_does_not_touch_carry() {
        let mut regs = Registers::reset();
        regs.set_flag(psr::C, true);
        // MOV r0, #0
        let op = (0b001 << 13) | (0b00 << 11) | (0 << 8) | 0;
        immediate_op(&mut regs, op);
        assert_eq!(regs.r(0), 0);
        assert!(regs.flag(psr::Z));
        assert!(regs.flag(psr::C), "MOV must not affect C");
    }

    #[test]
    fn cmp_immediate_sets_flags_without_writing_rd() {
        let mut regs = Registers::reset();
        regs.set_r(0, 5);
        // CMP r0, #5
        let op = (0b001 << 13) | (0b01 << 11) | (0 << 8) | 5;
        immediate_op(&mut regs, op);
        assert!(regs.flag(psr::Z));
        assert_eq!(regs.r(0), 5, "CMP must not write Rd");
    }

    #[test]
    fn alu_and_updates_nz_only() {
        let mut regs = Registers::reset();
        regs.set_flag(psr::C, true);
        regs.set_r(0, 0xFF);
        regs.set_r(1, 0x0F);
        // AND r0, r1
        let op = (0b010000 << 10) | (0x0 << 6) | (1 << 3) | 0;
        alu_operation(&mut regs, op);
        assert_eq!(regs.r(0), 0x0F);
        assert!(regs.flag(psr::C), "AND must not affect C in Thumb Format 4");
    }

    #[test]
    fn alu_mul_multiplies() {
        let mut regs = Registers::reset();
        regs.set_r(0, 6);
        regs.set_r(1, 7);
        // MUL r0, r1
        let op = (0b010000 << 10) | (0xD << 6) | (1 << 3) | 0;
        alu_operation(&mut regs, op);
        assert_eq!(regs.r(0), 42);
    }

    #[test]
    fn alu_neg_negates() {
        let mut regs = Registers::reset();
        regs.set_r(1, 5);
        // NEG r0, r1
        let op = (0b010000 << 10) | (0x9 << 6) | (1 << 3) | 0;
        alu_operation(&mut regs, op);
        assert_eq!(regs.r(0), (-5i32) as u32);
    }
}
