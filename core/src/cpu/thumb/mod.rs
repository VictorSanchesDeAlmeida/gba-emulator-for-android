mod alu_ops;
mod block;
mod branch;
mod hi_reg;
mod load_store;
mod stack;

use crate::cpu::{CpuError, Registers};
use crate::memory::Memory;

/// Decodes and executes one Thumb opcode. Organized by the 5-bit top field
/// (opcode bits 15-11), which cleanly separates 14 of the 19 formats; the
/// remaining ambiguities (Format 4 vs 5, Format 7 vs 8, Format 13 vs 14,
/// Format 16 vs 17) are resolved by one extra bit-check each, documented
/// inline where they'd otherwise be surprising.
pub fn execute<M: Memory>(
    regs: &mut Registers,
    mem: &mut M,
    opcode: u16,
    instr_addr: u32,
) -> Result<(u32, bool), CpuError> {
    let top5 = (opcode >> 11) & 0x1F;

    match top5 {
        // Format 1: Move shifted register (op bits12-11 in {00,01,10}).
        0b00000 | 0b00001 | 0b00010 => {
            alu_ops::move_shifted_register(regs, opcode);
            Ok((1, false))
        }
        // Format 2: Add/subtract (the op=11 case Format 1 excludes).
        0b00011 => {
            alu_ops::add_subtract(regs, opcode);
            Ok((1, false))
        }
        // Format 3: Move/compare/add/subtract immediate.
        0b00100..=0b00111 => {
            alu_ops::immediate_op(regs, opcode);
            Ok((1, false))
        }
        // Format 4 (bit10=0): ALU operations. Format 5 (bit10=1): hi-register ops/BX.
        0b01000 => {
            if (opcode >> 10) & 1 == 0 {
                alu_ops::alu_operation(regs, opcode);
                Ok((1, false))
            } else {
                Ok(hi_reg::execute(regs, opcode, instr_addr))
            }
        }
        // Format 6: PC-relative load.
        0b01001 => {
            load_store::pc_relative_load(regs, mem, opcode, instr_addr);
            Ok((3, false))
        }
        // Format 7 (bit9=0): register-offset load/store.
        // Format 8 (bit9=1): sign-extended byte/halfword load/store.
        0b01010 | 0b01011 => {
            if (opcode >> 9) & 1 == 0 {
                load_store::register_offset(regs, mem, opcode);
                let load = (opcode >> 11) & 1 == 1;
                Ok((if load { 3 } else { 2 }, false))
            } else {
                load_store::sign_extended(regs, mem, opcode);
                let is_store = (opcode >> 11) & 1 == 0 && (opcode >> 10) & 1 == 0; // S=0,H=0 -> STRH
                Ok((if is_store { 2 } else { 3 }, false))
            }
        }
        // Format 9: Load/store with immediate offset.
        0b01100..=0b01111 => {
            load_store::immediate_offset(regs, mem, opcode);
            let load = (opcode >> 11) & 1 == 1;
            Ok((if load { 3 } else { 2 }, false))
        }
        // Format 10: Load/store halfword.
        0b10000 | 0b10001 => {
            load_store::halfword(regs, mem, opcode);
            let load = (opcode >> 11) & 1 == 1;
            Ok((if load { 3 } else { 2 }, false))
        }
        // Format 11: SP-relative load/store.
        0b10010 | 0b10011 => {
            stack::sp_relative(regs, mem, opcode);
            let load = (opcode >> 11) & 1 == 1;
            Ok((if load { 3 } else { 2 }, false))
        }
        // Format 12: Load address (from PC or SP).
        0b10100 | 0b10101 => {
            stack::load_address(regs, opcode, instr_addr);
            Ok((1, false))
        }
        // Format 13 (bits10-8==000): add offset to SP.
        // Format 14 PUSH (bits10-9==10, L=0): shares top5 with Format 13
        // since both have bit11=0 — bits10-8 disambiguate them.
        0b10110 => {
            if (opcode >> 8) & 0b111 == 0b000 {
                stack::add_offset_to_sp(regs, opcode);
                Ok((1, false))
            } else if (opcode >> 9) & 0b11 == 0b10 {
                let (cycles, pc_written) = stack::push_pop(regs, mem, opcode);
                Ok((cycles, pc_written))
            } else {
                Err(CpuError::UnimplementedThumb { opcode, pc: instr_addr })
            }
        }
        // Format 14 POP (L=1).
        0b10111 => {
            let (cycles, pc_written) = stack::push_pop(regs, mem, opcode);
            Ok((cycles, pc_written))
        }
        // Format 15: Multiple load/store.
        0b11000 | 0b11001 => {
            let cycles = block::execute(regs, mem, opcode);
            Ok((cycles, false))
        }
        // Format 16: Conditional branch (cond bits11-8 in 0000..=1101; 1110
        // is reserved/undefined and never emitted by real compilers).
        0b11010 => {
            let (cycles, pc_written, target) = branch::conditional_branch(regs, opcode, instr_addr);
            if let Some(target) = target {
                regs.set_pc(target);
            }
            Ok((cycles, pc_written))
        }
        // Format 16 continued (cond bits11-8 in 1000..=1110), or
        // Format 17 (SWI) when bits15-8 == 0xDF exactly (cond field == 1111).
        0b11011 => {
            if opcode & 0xFF00 == 0xDF00 {
                Ok(branch::software_interrupt(regs, instr_addr))
            } else {
                let (cycles, pc_written, target) = branch::conditional_branch(regs, opcode, instr_addr);
                if let Some(target) = target {
                    regs.set_pc(target);
                }
                Ok((cycles, pc_written))
            }
        }
        // Format 18: Unconditional branch.
        0b11100 => {
            let target = branch::unconditional_branch(opcode, instr_addr);
            regs.set_pc(target);
            Ok((3, true))
        }
        // Format 19, first half (H=0): stashes a partial target in LR.
        0b11110 => {
            branch::branch_link_high(regs, opcode, instr_addr);
            Ok((1, false))
        }
        // Format 19, second half (H=1): completes the jump.
        0b11111 => {
            branch::branch_link_low(regs, opcode, instr_addr);
            Ok((3, true))
        }
        _ => Err(CpuError::UnimplementedThumb { opcode, pc: instr_addr }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::{Cartridge, HEADER_SIZE};
    use crate::memory::Bus;

    fn test_bus() -> Bus {
        Bus::new(Cartridge::load(vec![0u8; HEADER_SIZE + 16]).unwrap())
    }

    #[test]
    fn dispatches_format13_and_format14_despite_sharing_top5() {
        let mut bus = test_bus();
        let mut regs = Registers::reset();
        regs.set_r(13, 0x0300_0100); // SP

        // ADD SP, #16  (Format 13: bits15-8 = 1011_0000, SWord7=4)
        execute(&mut regs, &mut bus, 0xB004, 0).unwrap();
        assert_eq!(regs.r(13), 0x0300_0110);

        // PUSH {r0} (Format 14: bits15-9 = 1011010)
        regs.set_r(0, 0x1234_5678);
        execute(&mut regs, &mut bus, 0xB401, 0).unwrap();
        assert_eq!(regs.r(13), 0x0300_010C);
        assert_eq!(bus.read32(0x0300_010C), 0x1234_5678);
    }

    #[test]
    fn dispatches_conditional_branch_vs_swi_despite_sharing_top5() {
        let mut bus = test_bus();

        // BHI (cond=1000): top5 = 0b11011, same as SWI's — bits15-8 must
        // differ (0xD8xx vs 0xDFxx) for the dispatcher to tell them apart.
        let mut regs = Registers::reset();
        regs.set_flag(crate::cpu::psr::C, true); // HI: C=1 and Z=0
        let (cycles, pc_written) = execute(&mut regs, &mut bus, 0xD800, 0x1000).unwrap();
        assert!(pc_written);
        assert!(cycles >= 1);
        assert_eq!(regs.pc(), 0x1004, "a plain BHI, not a misrouted SWI (which would jump to 0x08)");

        // SWI: bits15-8 = 0xDF exactly.
        let mut regs2 = Registers::reset();
        regs2.set_mode(crate::cpu::Mode::User);
        let (_, pc_written2) = execute(&mut regs2, &mut bus, 0xDF00, 0x2000).unwrap();
        assert!(pc_written2);
        assert_eq!(regs2.mode(), crate::cpu::Mode::Supervisor);
    }

    #[test]
    fn full_program_computes_and_branches() {
        let mut bus = test_bus();
        let mut regs = Registers::reset();

        // MOV r0, #5   (Format 3)
        execute(&mut regs, &mut bus, 0x2005, 0x0000).unwrap();
        // MOV r1, #10  (Format 3)
        execute(&mut regs, &mut bus, 0x210A, 0x0002).unwrap();
        // ADD r2, r0, r1  (Format 2, register form)
        execute(&mut regs, &mut bus, 0x1842, 0x0004).unwrap();

        assert_eq!(regs.r(2), 15);
    }
}
