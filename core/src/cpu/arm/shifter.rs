/// The barrel shifter that computes a data-processing instruction's
/// operand2 (and its carry-out, which feeds CPSR C when the instruction
/// sets flags). See ARM7TDMI Data Sheet, "Shifts".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShifterResult {
    pub value: u32,
    pub carry_out: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftType {
    Lsl,
    Lsr,
    Asr,
    Ror,
}

impl ShiftType {
    pub fn from_bits(bits: u32) -> ShiftType {
        match bits & 0b11 {
            0b00 => ShiftType::Lsl,
            0b01 => ShiftType::Lsr,
            0b10 => ShiftType::Asr,
            0b11 => ShiftType::Ror,
            _ => unreachable!(),
        }
    }
}

/// Applies `shift_type` by `amount` to `value`.
///
/// `amount == 0` means two different things depending on how the
/// instruction encoded it, which is why the caller must say so:
/// - **Immediate shifts** (`is_immediate = true`) can't encode "shift by
///   zero" for LSR/ASR/ROR — the assembler instead encodes those as
///   LSR#32, ASR#32 and RRX (rotate through carry by 1) respectively. LSL#0
///   genuinely means no shift.
/// - **Register-specified shifts** (`is_immediate = false`) can encode a
///   literal zero (Rs low byte == 0), which ARM defines as "no shift at
///   all" for every shift type, carry flag included.
pub fn shift(shift_type: ShiftType, value: u32, amount: u32, carry_in: bool, is_immediate: bool) -> ShifterResult {
    match shift_type {
        ShiftType::Lsl => lsl(value, amount, carry_in),
        ShiftType::Lsr => lsr(value, amount, carry_in, is_immediate),
        ShiftType::Asr => asr(value, amount, carry_in, is_immediate),
        ShiftType::Ror => ror(value, amount, carry_in, is_immediate),
    }
}

fn lsl(value: u32, amount: u32, carry_in: bool) -> ShifterResult {
    match amount {
        0 => ShifterResult { value, carry_out: carry_in },
        1..=31 => ShifterResult {
            value: value << amount,
            carry_out: (value >> (32 - amount)) & 1 == 1,
        },
        32 => ShifterResult { value: 0, carry_out: value & 1 == 1 },
        _ => ShifterResult { value: 0, carry_out: false },
    }
}

fn lsr(value: u32, amount: u32, carry_in: bool, is_immediate: bool) -> ShifterResult {
    let effective = if amount == 0 && is_immediate { 32 } else { amount };
    match effective {
        0 => ShifterResult { value, carry_out: carry_in },
        1..=31 => ShifterResult {
            value: value >> effective,
            carry_out: (value >> (effective - 1)) & 1 == 1,
        },
        32 => ShifterResult { value: 0, carry_out: (value >> 31) & 1 == 1 },
        _ => ShifterResult { value: 0, carry_out: false },
    }
}

fn asr(value: u32, amount: u32, carry_in: bool, is_immediate: bool) -> ShifterResult {
    let effective = if amount == 0 && is_immediate { 32 } else { amount };
    let signed = value as i32;
    match effective {
        0 => ShifterResult { value, carry_out: carry_in },
        1..=31 => ShifterResult {
            value: (signed >> effective) as u32,
            carry_out: (value >> (effective - 1)) & 1 == 1,
        },
        _ => {
            // >= 32: fully sign-extended, further shifting changes nothing.
            let all_ones = signed < 0;
            ShifterResult {
                value: if all_ones { 0xFFFF_FFFF } else { 0 },
                carry_out: (value >> 31) & 1 == 1,
            }
        }
    }
}

fn ror(value: u32, amount: u32, carry_in: bool, is_immediate: bool) -> ShifterResult {
    if amount == 0 && is_immediate {
        // RRX: rotate right through carry by exactly 1 bit.
        return ShifterResult {
            value: (value >> 1) | ((carry_in as u32) << 31),
            carry_out: value & 1 == 1,
        };
    }
    if amount == 0 {
        return ShifterResult { value, carry_out: carry_in };
    }
    let effective = amount & 31;
    if effective == 0 {
        // Nonzero multiple of 32: value unchanged, carry = bit 31.
        ShifterResult { value, carry_out: (value >> 31) & 1 == 1 }
    } else {
        ShifterResult {
            value: value.rotate_right(effective),
            carry_out: (value >> (effective - 1)) & 1 == 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsl_by_zero_is_a_no_op() {
        let r = lsl(0x1234, 0, true);
        assert_eq!(r.value, 0x1234);
        assert!(r.carry_out);
    }

    #[test]
    fn lsl_by_32_zeroes_the_value() {
        let r = lsl(0b1, 32, false);
        assert_eq!(r.value, 0);
        assert!(r.carry_out, "carry_out is the bit shifted out, i.e. original bit 0");
    }

    #[test]
    fn lsl_by_more_than_32_is_all_zero() {
        let r = lsl(0xFFFF_FFFF, 33, true);
        assert_eq!(r.value, 0);
        assert!(!r.carry_out);
    }

    #[test]
    fn immediate_lsr_zero_means_lsr_32() {
        let r = shift(ShiftType::Lsr, 0x8000_0000, 0, false, true);
        assert_eq!(r.value, 0);
        assert!(r.carry_out, "LSR#32 carry is original bit 31");
    }

    #[test]
    fn register_lsr_zero_is_a_no_op() {
        let r = shift(ShiftType::Lsr, 0x8000_0000, 0, true, false);
        assert_eq!(r.value, 0x8000_0000);
        assert!(r.carry_out);
    }

    #[test]
    fn immediate_asr_zero_means_asr_32_sign_extends() {
        let r = shift(ShiftType::Asr, 0x8000_0000, 0, false, true);
        assert_eq!(r.value, 0xFFFF_FFFF);
        assert!(r.carry_out);

        let r_pos = shift(ShiftType::Asr, 0x1, 0, false, true);
        assert_eq!(r_pos.value, 0);
        assert!(!r_pos.carry_out);
    }

    #[test]
    fn asr_preserves_sign_for_negative_values() {
        let r = shift(ShiftType::Asr, 0x8000_0000u32, 4, false, false);
        assert_eq!(r.value, 0xF800_0000);
    }

    #[test]
    fn immediate_ror_zero_is_rrx() {
        let r = shift(ShiftType::Ror, 0b10, 0, true, true); // carry_in=1
        assert_eq!(r.value, 0x8000_0001);
        assert!(!r.carry_out, "bit 0 of the original value shifted out");
    }

    #[test]
    fn register_ror_zero_is_a_no_op() {
        let r = shift(ShiftType::Ror, 0x1234, 0, true, false);
        assert_eq!(r.value, 0x1234);
        assert!(r.carry_out);
    }

    #[test]
    fn ror_by_nonzero_multiple_of_32_leaves_value_unchanged() {
        let r = shift(ShiftType::Ror, 0x8000_0001, 32, false, false);
        assert_eq!(r.value, 0x8000_0001);
        assert!(r.carry_out, "carry becomes bit 31");
    }

    #[test]
    fn ror_rotates_and_reports_last_bit_rotated_as_carry() {
        let r = shift(ShiftType::Ror, 0b1, 1, false, false);
        assert_eq!(r.value, 0x8000_0000);
        assert!(r.carry_out);
    }
}
