use crate::cpu::{Mode, Registers};

const FLAGS_MASK: u32 = 0xFF00_0000; // bit19 of the field-mask ("f")
const CONTROL_MASK: u32 = 0x0000_00FF; // bit16 of the field-mask ("c")
// Field-mask bits 17 ("x") and 18 ("s") address PSR bytes ARMv4T doesn't
// define any bits in, so they're accepted but have no effect here.

/// MRS/MSR: moves the whole CPSR/SPSR to a register, or writes selected
/// byte-fields of it from a register or rotated immediate.
pub fn execute(regs: &mut Registers, opcode: u32) -> (u32, bool) {
    let use_spsr = (opcode >> 22) & 1 == 1;
    let is_msr = (opcode >> 21) & 1 == 1;

    if !is_msr {
        let rd = ((opcode >> 12) & 0xF) as usize;
        let value = if use_spsr { regs.spsr() } else { regs.cpsr() };
        regs.set_r(rd, value);
        return (1, false);
    }

    let field_mask = (opcode >> 16) & 0xF;
    let mut mask = 0u32;
    if field_mask & 0b1000 != 0 {
        mask |= FLAGS_MASK;
    }
    if field_mask & 0b0001 != 0 {
        mask |= CONTROL_MASK;
    }

    let immediate = (opcode >> 25) & 1 == 1;
    let source = if immediate {
        let imm8 = opcode & 0xFF;
        let rotate = ((opcode >> 8) & 0xF) * 2;
        imm8.rotate_right(rotate)
    } else {
        let rm = (opcode & 0xF) as usize;
        regs.r(rm)
    };

    // User mode is only privileged to change the flags field, never
    // mode/I/F/T — real hardware silently drops the rest.
    if !use_spsr && regs.mode() == Mode::User {
        mask &= FLAGS_MASK;
    }

    if use_spsr {
        let merged = (regs.spsr() & !mask) | (source & mask);
        regs.set_spsr(merged);
    } else {
        let merged = (regs.cpsr() & !mask) | (source & mask);
        regs.set_cpsr(merged);
    }

    (1, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::psr;

    fn encode_mrs(use_spsr: bool, rd: u32) -> u32 {
        (0xE << 28) | (0b00010 << 23) | ((use_spsr as u32) << 22) | (0b1111 << 16) | (rd << 12)
    }

    fn encode_msr_register(use_spsr: bool, field_mask: u32, rm: u32) -> u32 {
        (0xE << 28) | (0b00010 << 23) | ((use_spsr as u32) << 22) | (1 << 21) | (field_mask << 16) | (0b1111 << 12) | rm
    }

    fn encode_msr_immediate(use_spsr: bool, field_mask: u32, rotate: u32, imm8: u32) -> u32 {
        (0xE << 28) | (1 << 25) | (0b10 << 23) | ((use_spsr as u32) << 22) | (1 << 21)
            | (field_mask << 16) | (0b1111 << 12) | (rotate << 8) | imm8
    }

    #[test]
    fn mrs_reads_cpsr_into_a_register() {
        let mut regs = Registers::reset();
        execute(&mut regs, encode_mrs(false, 0));
        assert_eq!(regs.r(0), regs.cpsr());
    }

    #[test]
    fn msr_immediate_flags_only_touches_top_byte() {
        let mut regs = Registers::reset();
        let before = regs.cpsr();
        // field_mask=0b1000 (flags only). Rotate field 4 -> actual rotation
        // 8 (field*2), placing imm8=0xF0 at bits 31:24
        // (0xF0u32.rotate_right(8) == 0xF000_0000).
        let op = encode_msr_immediate(false, 0b1000, 4, 0xF0);
        execute(&mut regs, op);
        assert_eq!(regs.cpsr() & FLAGS_MASK, 0xF000_0000);
        assert_eq!(regs.cpsr() & !FLAGS_MASK, before & !FLAGS_MASK, "control byte untouched");
    }

    #[test]
    fn msr_register_control_field_changes_mode() {
        let mut regs = Registers::reset(); // starts Supervisor
        regs.set_r(0, Mode::Irq.bits() | psr::I | psr::F);
        let op = encode_msr_register(false, 0b0001, 0); // control field only
        execute(&mut regs, op);
        assert_eq!(regs.mode(), Mode::Irq);
    }

    #[test]
    fn user_mode_cannot_change_control_field_via_msr() {
        let mut regs = Registers::reset();
        regs.set_mode(Mode::User);
        regs.set_r(0, Mode::Supervisor.bits());
        let op = encode_msr_register(false, 0b1001, 0); // asks for flags+control
        execute(&mut regs, op);
        assert_eq!(regs.mode(), Mode::User, "control field write must be dropped in User mode");
    }

    #[test]
    fn msr_to_spsr_does_not_touch_cpsr() {
        let mut regs = Registers::reset();
        regs.set_mode(Mode::Irq);
        let before_cpsr = regs.cpsr();
        regs.set_r(0, 0xFFFF_FFFF);
        // field_mask=0b1001 (flags + control — the two fields ARMv4T
        // defines and this backend implements; bits 1/2 are documented
        // no-ops, see FLAGS_MASK/CONTROL_MASK above).
        let op = encode_msr_register(true, 0b1001, 0);
        execute(&mut regs, op);
        assert_eq!(regs.cpsr(), before_cpsr);
        assert_eq!(regs.spsr(), FLAGS_MASK | CONTROL_MASK);
    }
}
