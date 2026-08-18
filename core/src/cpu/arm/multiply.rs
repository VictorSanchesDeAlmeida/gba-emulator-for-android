use crate::cpu::{psr, Registers};

/// MUL/MLA: Rd = Rm * Rs (+ Rn if accumulating). Real hardware forbids
/// Rd == Rm and using r15 as any operand (UNPREDICTABLE); neither is
/// specially guarded here since well-formed code never does either.
pub fn execute_mul(regs: &mut Registers, opcode: u32) -> (u32, bool) {
    let s = (opcode >> 20) & 1 == 1;
    let accumulate = (opcode >> 21) & 1 == 1;
    let rd = ((opcode >> 16) & 0xF) as usize;
    let rn = ((opcode >> 12) & 0xF) as usize;
    let rs = ((opcode >> 8) & 0xF) as usize;
    let rm = (opcode & 0xF) as usize;

    let product = regs.r(rm).wrapping_mul(regs.r(rs));
    let result = if accumulate { product.wrapping_add(regs.r(rn)) } else { product };

    regs.set_r(rd, result);
    if s {
        regs.set_flag(psr::Z, result == 0);
        regs.set_flag(psr::N, (result >> 31) & 1 == 1);
        // C is architecturally meaningless after MUL/MLA on ARMv4; left as-is.
    }
    (1, false) // real cost varies with operand magnitude; refined in Etapa 5
}

/// UMULL/UMLAL/SMULL/SMLAL: 64-bit product (optionally accumulated) split
/// across RdHi:RdLo.
pub fn execute_mul_long(regs: &mut Registers, opcode: u32) -> (u32, bool) {
    let signed = (opcode >> 22) & 1 == 1;
    let accumulate = (opcode >> 21) & 1 == 1;
    let s = (opcode >> 20) & 1 == 1;
    let rd_hi = ((opcode >> 16) & 0xF) as usize;
    let rd_lo = ((opcode >> 12) & 0xF) as usize;
    let rs = ((opcode >> 8) & 0xF) as usize;
    let rm = (opcode & 0xF) as usize;

    let mut result: u64 = if signed {
        (regs.r(rm) as i32 as i64).wrapping_mul(regs.r(rs) as i32 as i64) as u64
    } else {
        (regs.r(rm) as u64).wrapping_mul(regs.r(rs) as u64)
    };

    if accumulate {
        let acc = ((regs.r(rd_hi) as u64) << 32) | regs.r(rd_lo) as u64;
        result = result.wrapping_add(acc);
    }

    regs.set_r(rd_lo, (result & 0xFFFF_FFFF) as u32);
    regs.set_r(rd_hi, (result >> 32) as u32);

    if s {
        regs.set_flag(psr::Z, result == 0);
        regs.set_flag(psr::N, (result >> 63) & 1 == 1);
    }
    (1, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_mul(cond: u32, accumulate: bool, s: bool, rd: u32, rn: u32, rs: u32, rm: u32) -> u32 {
        (cond << 28) | ((accumulate as u32) << 21) | ((s as u32) << 20) | (rd << 16) | (rn << 12) | (rs << 8) | (0b1001 << 4) | rm
    }

    fn encode_mul_long(cond: u32, signed: bool, accumulate: bool, s: bool, rd_hi: u32, rd_lo: u32, rs: u32, rm: u32) -> u32 {
        (cond << 28) | (1 << 23) | ((signed as u32) << 22) | ((accumulate as u32) << 21) | ((s as u32) << 20)
            | (rd_hi << 16) | (rd_lo << 12) | (rs << 8) | (0b1001 << 4) | rm
    }

    #[test]
    fn mul_multiplies_two_registers() {
        let mut regs = Registers::reset();
        regs.set_r(1, 6);
        regs.set_r(2, 7);
        let op = encode_mul(0xE, false, true, 0, 0, 2, 1); // MULS r0, r1, r2
        execute_mul(&mut regs, op);
        assert_eq!(regs.r(0), 42);
        assert!(!regs.flag(psr::Z));
    }

    #[test]
    fn mla_accumulates() {
        let mut regs = Registers::reset();
        regs.set_r(1, 3);
        regs.set_r(2, 4);
        regs.set_r(3, 100);
        let op = encode_mul(0xE, true, false, 0, 3, 2, 1); // MLA r0, r1, r2, r3
        execute_mul(&mut regs, op);
        assert_eq!(regs.r(0), 112);
    }

    #[test]
    fn umull_splits_a_64_bit_product() {
        let mut regs = Registers::reset();
        regs.set_r(2, 0xFFFF_FFFF);
        regs.set_r(3, 2);
        let op = encode_mul_long(0xE, false, false, false, 0, 1, 3, 2); // UMULL r1(lo), r0(hi), r2, r3
        execute_mul_long(&mut regs, op);
        let result = ((regs.r(0) as u64) << 32) | regs.r(1) as u64;
        assert_eq!(result, 0xFFFF_FFFFu64 * 2);
    }

    #[test]
    fn smull_handles_negative_operands() {
        let mut regs = Registers::reset();
        regs.set_r(2, (-5i32) as u32);
        regs.set_r(3, (-3i32) as u32);
        let op = encode_mul_long(0xE, true, false, true, 0, 1, 3, 2); // SMULLS r1(lo), r0(hi), r2, r3
        execute_mul_long(&mut regs, op);
        let result = ((regs.r(0) as u64) << 32) | regs.r(1) as u64;
        assert_eq!(result as i64, 15);
        assert!(!regs.flag(psr::N));
    }
}
