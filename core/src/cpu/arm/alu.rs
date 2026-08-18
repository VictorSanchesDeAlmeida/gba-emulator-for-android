/// Result of an ALU add/subtract: the value plus the carry/overflow it
/// produced, ready to feed into CPSR when the instruction sets flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AluResult {
    pub value: u32,
    pub carry: bool,
    pub overflow: bool,
}

/// `a + b + carry_in`, with the carry/overflow flags ARM defines for
/// addition: carry is unsigned overflow, overflow is signed overflow
/// (operands share a sign that the result doesn't).
pub fn add_with_carry(a: u32, b: u32, carry_in: bool) -> AluResult {
    let (r1, c1) = a.overflowing_add(b);
    let (value, c2) = r1.overflowing_add(carry_in as u32);
    let carry = c1 || c2;
    let overflow = (!(a ^ b) & (a ^ value)) >> 31 & 1 == 1;
    AluResult { value, carry, overflow }
}

/// `a - b - (1 - borrow_in)`, expressed as ARM's own hardware does it: via
/// the adder, using `sub(a, b) == add(a, !b, carry_in=1)`. ARM's carry flag
/// for subtraction means "no borrow occurred" (the inverse of a borrow
/// flag), which is exactly what reusing `add_with_carry` with `!b` and
/// `carry_in=true` produces for a plain subtract.
pub fn sub_with_carry(a: u32, b: u32, carry_in: bool) -> AluResult {
    add_with_carry(a, !b, carry_in)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_reports_unsigned_carry() {
        let r = add_with_carry(0xFFFF_FFFF, 1, false);
        assert_eq!(r.value, 0);
        assert!(r.carry);
        assert!(!r.overflow);
    }

    #[test]
    fn add_reports_signed_overflow() {
        // Two large positives overflowing into a negative result.
        let r = add_with_carry(0x7FFF_FFFF, 1, false);
        assert_eq!(r.value, 0x8000_0000);
        assert!(!r.carry);
        assert!(r.overflow);
    }

    #[test]
    fn plain_sub_uses_carry_in_true_for_no_borrow() {
        let r = sub_with_carry(5, 3, true);
        assert_eq!(r.value, 2);
        assert!(r.carry, "carry set means no borrow occurred");
    }

    #[test]
    fn sub_reports_borrow_as_cleared_carry() {
        let r = sub_with_carry(3, 5, true);
        assert_eq!(r.value, 3u32.wrapping_sub(5));
        assert!(!r.carry, "borrow occurred, so carry is clear");
    }

    #[test]
    fn sbc_folds_in_the_previous_carry() {
        // 5 - 3 - (1 - C); with C=0 this subtracts one extra.
        let r = sub_with_carry(5, 3, false);
        assert_eq!(r.value, 1);
    }

    #[test]
    fn sub_reports_signed_overflow() {
        let r = sub_with_carry(0x8000_0000, 1, true);
        assert_eq!(r.value, 0x7FFF_FFFF);
        assert!(r.overflow);
    }
}
