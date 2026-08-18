pub(super) mod alu;
mod block_transfer;
mod branch;
mod branch_exchange;
mod data_processing;
mod halfword_transfer;
mod multiply;
mod psr_transfer;
pub(super) mod shifter;
mod single_transfer;
mod swap;
mod swi;

use super::{psr, CpuError, Registers};
use crate::memory::Memory;

/// Evaluates an ARM condition code (opcode bits 31-28) against the current
/// flags. Every ARM instruction is conditional; `step` skips execution
/// (but not the fetch) when this is false.
pub(super) fn check_condition(cond: u32, regs: &Registers) -> bool {
    let n = regs.flag(psr::N);
    let z = regs.flag(psr::Z);
    let c = regs.flag(psr::C);
    let v = regs.flag(psr::V);
    match cond & 0xF {
        0x0 => z,               // EQ
        0x1 => !z,              // NE
        0x2 => c,               // CS/HS
        0x3 => !c,              // CC/LO
        0x4 => n,               // MI
        0x5 => !n,              // PL
        0x6 => v,               // VS
        0x7 => !v,              // VC
        0x8 => c && !z,         // HI
        0x9 => !c || z,         // LS
        0xA => n == v,          // GE
        0xB => n != v,          // LT
        0xC => !z && (n == v),  // GT
        0xD => z || (n != v),   // LE
        0xE => true,            // AL
        _ => false,              // NV (reserved) — never executes
    }
}

/// Decodes and executes one ARM opcode. Returns `(cycles, pc_written)`
/// where `pc_written` tells the caller not to auto-advance PC because the
/// instruction already set it (a taken branch, or any instruction whose
/// destination register was r15).
///
/// Organized by opcode *group* (condition -> bits 27-25 -> sub-pattern),
/// not a single flat switch over all 4096 possible bit patterns — each
/// group's decode/execute logic lives in its own module.
pub fn execute<M: Memory>(
    regs: &mut Registers,
    mem: &mut M,
    opcode: u32,
    instr_addr: u32,
) -> Result<(u32, bool), CpuError> {
    let cond = opcode >> 28;
    if !check_condition(cond, regs) {
        return Ok((1, false)); // condition failed: fetched, not executed
    }

    let group = (opcode >> 25) & 0b111;
    match group {
        0b101 => Ok(branch::execute(regs, opcode, instr_addr)),

        0b000 => {
            if is_branch_and_exchange(opcode) {
                Ok(branch_exchange::execute(regs, opcode, instr_addr))
            } else if is_multiply(opcode) {
                Ok(multiply::execute_mul(regs, opcode))
            } else if is_multiply_long(opcode) {
                Ok(multiply::execute_mul_long(regs, opcode))
            } else if is_swap(opcode) {
                Ok(swap::execute(regs, mem, opcode))
            } else if is_halfword_transfer(opcode) {
                // Must be checked before is_psr_transfer: a halfword
                // transfer whose P/U/I/W bits happen to land its Rn field
                // in MRS/MSR's TST..CMN range with S=0 satisfies that
                // predicate too — same class of collision is_swap is
                // already disambiguated above. Real MRS/MSR always has
                // bits7-4 == 0000, which never matches this predicate, so
                // checking it first only steals genuine halfword-transfer
                // encodings, never genuine PSR-transfer ones.
                Ok(halfword_transfer::execute(regs, mem, opcode, instr_addr))
            } else if is_psr_transfer(opcode) {
                Ok(psr_transfer::execute(regs, opcode))
            } else {
                Ok(data_processing::execute(regs, opcode, instr_addr))
            }
        }

        0b001 => {
            if is_psr_transfer(opcode) {
                Ok(psr_transfer::execute(regs, opcode))
            } else {
                Ok(data_processing::execute(regs, opcode, instr_addr))
            }
        }

        0b010 => Ok(single_transfer::execute(regs, mem, opcode, instr_addr)),

        0b011 => {
            if (opcode >> 4) & 1 == 1 {
                // Register-offset LDR/STR with bit4=1 is a reserved
                // (undefined-instruction) encoding on ARMv4.
                Err(CpuError::UnimplementedArm { opcode, pc: instr_addr })
            } else {
                Ok(single_transfer::execute(regs, mem, opcode, instr_addr))
            }
        }

        0b100 => Ok(block_transfer::execute(regs, mem, opcode, instr_addr)),

        0b111 if (opcode >> 24) & 1 == 1 => Ok(swi::execute(regs, instr_addr)),

        // Coprocessor data transfer / data operation / register transfer
        // (0b110, or 0b111 with bit24=0): the GBA has no coprocessor, so
        // real software never generates these.
        _ => Err(CpuError::UnimplementedArm { opcode, pc: instr_addr }),
    }
}

/// True for the PSR-transfer (MRS/MSR) encodings, which live in the same
/// bit-27-25 == 00 space as Data Processing but are *not* Data Processing:
/// TST/TEQ/CMP/CMN with S=0 is reserved for MRS/MSR, not a flag-less
/// compare. Must be checked after [`is_swap`] — a genuine SWP instruction
/// (bit21=0, bit20=0) has identical bits 27-23 to MRS and would otherwise
/// be misclassified; SWP's exact bits7-4==1001 requirement never occurs in
/// a well-formed MRS/MSR encoding, so checking it first disambiguates them.
fn is_psr_transfer(opcode: u32) -> bool {
    let dp_opcode = (opcode >> 21) & 0xF;
    let s = (opcode >> 20) & 1;
    let is_compare = (0x8..=0xB).contains(&dp_opcode);
    is_compare && s == 0 && (opcode >> 26) & 0b11 == 0b00
}

fn is_branch_and_exchange(opcode: u32) -> bool {
    (opcode & 0x0FFF_FFF0) == 0x012F_FF10
}

/// MUL/MLA: bits27-22 all zero, bits7-4 == 1001.
fn is_multiply(opcode: u32) -> bool {
    (opcode >> 22) & 0x3F == 0 && (opcode >> 4) & 0xF == 0b1001
}

/// UMULL/UMLAL/SMULL/SMLAL: bits27-23 == 00001, bits7-4 == 1001.
fn is_multiply_long(opcode: u32) -> bool {
    (opcode >> 23) & 0x1F == 0b00001 && (opcode >> 4) & 0xF == 0b1001
}

/// SWP/SWPB: bits27-23 == 00010, bits21-20 == 00, bits7-4 == 1001.
fn is_swap(opcode: u32) -> bool {
    (opcode >> 23) & 0x1F == 0b00010 && (opcode >> 20) & 0b11 == 0 && (opcode >> 4) & 0xF == 0b1001
}

/// LDRH/STRH/LDRSB/LDRSH: bit25=0 (I=0, only this group's own I-bit meaning
/// applies), bits7-4 has bit7=1 and bit4=1 but isn't exactly 1001 (that's
/// Multiply/Swap territory, already excluded by decode order).
fn is_halfword_transfer(opcode: u32) -> bool {
    let bits7_4 = (opcode >> 4) & 0xF;
    (opcode >> 25) & 1 == 0 && bits7_4 & 0b1001 == 0b1001 && bits7_4 != 0b1001
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regs_with_flags(n: bool, z: bool, c: bool, v: bool) -> Registers {
        let mut regs = Registers::reset();
        regs.set_flag(psr::N, n);
        regs.set_flag(psr::Z, z);
        regs.set_flag(psr::C, c);
        regs.set_flag(psr::V, v);
        regs
    }

    #[test]
    fn eq_and_ne_follow_zero_flag() {
        let regs = regs_with_flags(false, true, false, false);
        assert!(check_condition(0x0, &regs));
        assert!(!check_condition(0x1, &regs));
    }

    #[test]
    fn ge_lt_gt_le_follow_n_xor_v_and_z() {
        let regs = regs_with_flags(true, false, false, true);
        assert!(check_condition(0xA, &regs)); // GE
        assert!(!check_condition(0xB, &regs)); // LT
        assert!(check_condition(0xC, &regs)); // GT
        assert!(!check_condition(0xD, &regs)); // LE
    }

    #[test]
    fn al_is_always_true_and_nv_is_always_false() {
        let regs = Registers::reset();
        assert!(check_condition(0xE, &regs));
        assert!(!check_condition(0xF, &regs));
    }

    #[test]
    fn detects_psr_transfer_vs_data_processing() {
        let cmp = 0b1110_00_0_1010_1_0000_0000_0000_0000_0001; // CMP r0, r1 (S=1): real compare
        assert!(!is_psr_transfer(cmp));

        let mrs_like = 0b1110_00_0_1010_0_0000_0000_0000_0000_0001; // same field, S=0: reserved for MRS/MSR
        assert!(is_psr_transfer(mrs_like));
    }

    #[test]
    fn distinguishes_swp_from_mrs_despite_identical_bits_27_to_20() {
        // SWP r2, r1, [r0]: bits27-23=00010, bits21-20=00 — identical to
        // MRS's fixed fields. Only bits7-4 differ (1001 vs 0000), which is
        // exactly why the dispatcher must check is_swap before
        // is_psr_transfer: this opcode satisfies *both* predicates.
        let swp = 0xE100_2091u32;
        assert!(is_swap(swp));
        assert!(is_psr_transfer(swp), "same bit pattern also satisfies the PSR-transfer predicate");

        let mrs = 0xE10F_0000u32; // MRS r0, CPSR
        assert!(!is_swap(mrs));
        assert!(is_psr_transfer(mrs));
    }

    #[test]
    fn detects_branch_and_exchange() {
        let bx_r0 = 0xE12F_FF10u32;
        assert!(is_branch_and_exchange(bx_r0));
    }

    #[test]
    fn detects_multiply_and_multiply_long_and_excludes_data_processing() {
        let mul = 0xE000_0091u32; // MUL r0, r1, r0
        assert!(is_multiply(mul));

        let umull = 0xE080_0091u32; // UMULL r0, r8, r0, r1 (bits27-23=00001)
        assert!(is_multiply_long(umull));

        // A Data Processing register-shift form must never match either.
        let mov_lsl_reg = 0xE1A0_0211u32; // MOV r0, r1, LSL r2
        assert!(!is_multiply(mov_lsl_reg));
        assert!(!is_multiply_long(mov_lsl_reg));
        assert!(!is_halfword_transfer(mov_lsl_reg));
    }
}
