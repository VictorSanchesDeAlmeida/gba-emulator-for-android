use super::alu;
use super::shifter::{self, ShiftType, ShifterResult};
use crate::cpu::{psr, Registers};

/// Reads a register operand applying the ARM7TDMI's "PC read-ahead" quirk:
/// an instruction reads r15 as its own address + 8 normally, or +12 when
/// the instruction uses a register-specified shift amount (that encoding
/// takes an extra internal cycle, advancing the pipeline one word further).
fn read_operand(regs: &Registers, reg: usize, instr_addr: u32, register_specified_shift: bool) -> u32 {
    if reg == 15 {
        instr_addr.wrapping_add(if register_specified_shift { 12 } else { 8 })
    } else {
        regs.r(reg)
    }
}

/// Computes operand2 (immediate-rotate or shifted-register) and its
/// shifter carry-out, per ARM7TDMI Data Processing operand2 encoding.
fn compute_operand2(regs: &Registers, opcode: u32, instr_addr: u32) -> ShifterResult {
    let carry_in = regs.flag(psr::C);
    let immediate_operand2 = (opcode >> 25) & 1 == 1;

    if immediate_operand2 {
        let imm8 = opcode & 0xFF;
        let rotate = ((opcode >> 8) & 0xF) * 2;
        // Immediate operand2's own "rotate by 0" just means no rotation —
        // unlike the register-operand2 ROR#0 encoding, this is never RRX.
        if rotate == 0 {
            ShifterResult { value: imm8, carry_out: carry_in }
        } else {
            let value = imm8.rotate_right(rotate);
            ShifterResult { value, carry_out: (value >> 31) & 1 == 1 }
        }
    } else {
        let rm = (opcode & 0xF) as usize;
        let shift_type = ShiftType::from_bits(opcode >> 5);
        let register_specified = (opcode >> 4) & 1 == 1;

        let (amount, is_immediate) = if register_specified {
            let rs = ((opcode >> 8) & 0xF) as usize;
            let amount = read_operand(regs, rs, instr_addr, true) & 0xFF;
            (amount, false)
        } else {
            ((opcode >> 7) & 0x1F, true)
        };

        let rm_value = read_operand(regs, rm, instr_addr, register_specified);
        shifter::shift(shift_type, rm_value, amount, carry_in, is_immediate)
    }
}

/// Executes a Data Processing instruction (the 16 ALU/compare/move
/// opcodes). Returns `(cycles, pc_written)`; `pc_written` is true when the
/// instruction's destination register was r15.
pub fn execute(regs: &mut Registers, opcode: u32, instr_addr: u32) -> (u32, bool) {
    let s = (opcode >> 20) & 1 == 1;
    let rn = ((opcode >> 16) & 0xF) as usize;
    let rd = ((opcode >> 12) & 0xF) as usize;
    let dp_opcode = (opcode >> 21) & 0xF;
    let register_specified_shift = (opcode >> 25) & 1 == 0 && (opcode >> 4) & 1 == 1;

    let op2 = compute_operand2(regs, opcode, instr_addr);
    let rn_value = read_operand(regs, rn, instr_addr, register_specified_shift);
    let current_v = regs.flag(psr::V);

    // (result, carry, overflow, writes_result_to_rd)
    let (result, carry, overflow, writes_result) = match dp_opcode {
        0x0 => (rn_value & op2.value, op2.carry_out, current_v, true), // AND
        0x1 => (rn_value ^ op2.value, op2.carry_out, current_v, true), // EOR
        0x2 => sub(rn_value, op2.value, true, true),                  // SUB
        0x3 => sub(op2.value, rn_value, true, true),                  // RSB
        0x4 => add(rn_value, op2.value, false, true),                 // ADD
        0x5 => add(rn_value, op2.value, regs.flag(psr::C), true),     // ADC
        0x6 => sub(rn_value, op2.value, regs.flag(psr::C), true),     // SBC
        0x7 => sub(op2.value, rn_value, regs.flag(psr::C), true),     // RSC
        0x8 => (rn_value & op2.value, op2.carry_out, current_v, false), // TST
        0x9 => (rn_value ^ op2.value, op2.carry_out, current_v, false), // TEQ
        0xA => sub(rn_value, op2.value, true, false),                 // CMP
        0xB => add(rn_value, op2.value, false, false),                // CMN
        0xC => (rn_value | op2.value, op2.carry_out, current_v, true), // ORR
        0xD => (op2.value, op2.carry_out, current_v, true),           // MOV
        0xE => (rn_value & !op2.value, op2.carry_out, current_v, true), // BIC
        0xF => (!op2.value, op2.carry_out, current_v, true),          // MVN
        _ => unreachable!("4-bit field"),
    };

    if s {
        regs.set_flag(psr::Z, result == 0);
        regs.set_flag(psr::N, (result >> 31) & 1 == 1);
        regs.set_flag(psr::C, carry);
        regs.set_flag(psr::V, overflow);
    }

    if !writes_result {
        return (1, false);
    }

    regs.set_r(rd, result);
    if rd == 15 {
        if s {
            // MOVS/ADDS/... PC, ... is how exception handlers return:
            // restore CPSR (and re-bank registers) from the current SPSR.
            regs.set_cpsr(regs.spsr());
        }
        (3, true) // pipeline refill; refined once Etapa 5 models timing
    } else {
        (1, false)
    }
}

fn add(a: u32, b: u32, carry_in: bool, writes: bool) -> (u32, bool, bool, bool) {
    let r = alu::add_with_carry(a, b, carry_in);
    (r.value, r.carry, r.overflow, writes)
}

fn sub(a: u32, b: u32, carry_in: bool, writes: bool) -> (u32, bool, bool, bool) {
    let r = alu::sub_with_carry(a, b, carry_in);
    (r.value, r.carry, r.overflow, writes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_dp(cond: u32, opcode4: u32, s: u32, rn: u32, rd: u32, operand2: u32, immediate: bool) -> u32 {
        (cond << 28) | ((immediate as u32) << 25) | (opcode4 << 21) | (s << 20) | (rn << 16) | (rd << 12) | operand2
    }

    #[test]
    fn mov_immediate_sets_register_and_flags() {
        let mut regs = Registers::reset();
        // MOVS r0, #0 -> Z=1
        let op = encode_dp(0xE, 0xD, 1, 0, 0, 0, true);
        let (cycles, pc_written) = execute(&mut regs, op, 0x0000_0000);
        assert_eq!(regs.r(0), 0);
        assert!(regs.flag(psr::Z));
        assert_eq!(cycles, 1);
        assert!(!pc_written);
    }

    #[test]
    fn add_sets_carry_and_overflow() {
        let mut regs = Registers::reset();
        regs.set_r(1, 0xFFFF_FFFF);
        // ADDS r0, r1, #1
        let op = encode_dp(0xE, 0x4, 1, 1, 0, 1, true);
        execute(&mut regs, op, 0);
        assert_eq!(regs.r(0), 0);
        assert!(regs.flag(psr::C));
        assert!(regs.flag(psr::Z));
    }

    #[test]
    fn cmp_updates_flags_without_writing_rd() {
        let mut regs = Registers::reset();
        regs.set_r(0, 5);
        regs.set_r(1, 5);
        regs.set_r(2, 0xDEAD_BEEF);
        // CMP r0, r1 (register form, LSL#0) — Rd field is r2, must stay untouched.
        let op = encode_dp(0xE, 0xA, 1, 0, 2, 1, false);
        execute(&mut regs, op, 0);
        assert!(regs.flag(psr::Z), "5 - 5 == 0");
        assert_eq!(regs.r(2), 0xDEAD_BEEF, "CMP must not write Rd");
    }

    #[test]
    fn pc_read_as_operand_is_instruction_address_plus_8() {
        let mut regs = Registers::reset();
        // MOV r0, r15 (immediate-shift register form, Rm = r15)
        let op = encode_dp(0xE, 0xD, 0, 0, 0, 15, false);
        execute(&mut regs, op, 0x0000_1000);
        assert_eq!(regs.r(0), 0x0000_1008);
    }

    #[test]
    fn pc_read_with_register_specified_shift_is_plus_12() {
        let mut regs = Registers::reset();
        regs.set_r(2, 0); // shift amount register, Rs = r2, shift by 0 -> LSL#0 no-op
        // MOV r0, r15, LSL r2  (Rs=2 << 8 | shift_type=LSL(00) << 5 | bit4=1 | Rm=15)
        let operand2 = (2 << 8) | (0b00 << 5) | (1 << 4) | 15;
        let op = encode_dp(0xE, 0xD, 0, 0, 0, operand2, false);
        execute(&mut regs, op, 0x0000_1000);
        assert_eq!(regs.r(0), 0x0000_100C);
    }

    #[test]
    fn mov_pc_writes_pc_and_reports_pc_written() {
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0800_1234);
        // MOV r15, r0 (S=0)
        let op = encode_dp(0xE, 0xD, 0, 0, 15, 0, false);
        let (_, pc_written) = execute(&mut regs, op, 0);
        assert!(pc_written);
        assert_eq!(regs.pc(), 0x0800_1234);
    }

    #[test]
    fn movs_pc_restores_cpsr_from_spsr() {
        let mut regs = Registers::reset();
        regs.set_mode(crate::cpu::Mode::Irq);
        let user_cpsr = crate::cpu::Mode::User.bits(); // no I/F/T set: back to unprivileged ARM state
        regs.set_spsr(user_cpsr);
        regs.set_r(14, 0x0800_5678); // typical "MOVS PC, LR" return-from-IRQ idiom

        // MOVS r15, r14 (S=1)
        let op = encode_dp(0xE, 0xD, 1, 0, 15, 14, false);
        execute(&mut regs, op, 0);

        assert_eq!(regs.pc(), 0x0800_5678);
        assert_eq!(regs.mode(), crate::cpu::Mode::User);
    }

    #[test]
    fn immediate_rotate_of_zero_is_not_rrx() {
        let mut regs = Registers::reset();
        regs.set_flag(psr::C, true);
        // MOVS r0, #0x80 with rotate field = 0: must stay 0x80, C unaffected
        // by the rotate itself (only by the immediate's own carry_in passthrough).
        let op = encode_dp(0xE, 0xD, 1, 0, 0, 0x80, true);
        execute(&mut regs, op, 0);
        assert_eq!(regs.r(0), 0x80);
        assert!(regs.flag(psr::C), "rotate=0 leaves C unchanged, not RRX");
    }
}
