use crate::cpu::Registers;
use crate::memory::Memory;

const SP: usize = 13;
const LR: usize = 14;
const PC: usize = 15;

/// Format 11 — SP-relative load/store: `STR/LDR Rd, [SP, #Word8*4]`.
pub fn sp_relative<M: Memory>(regs: &mut Registers, mem: &mut M, opcode: u16) {
    let load = (opcode >> 11) & 1 == 1;
    let rd = ((opcode >> 8) & 0x7) as usize;
    let word8 = (opcode & 0xFF) as u32;
    let addr = regs.r(SP).wrapping_add(word8 * 4);

    if load {
        regs.set_r(rd, mem.read32(addr & !3).rotate_right((addr & 3) * 8));
    } else {
        mem.write32(addr & !3, regs.r(rd));
    }
}

/// Format 12 — Load address: `ADD Rd, PC, #Word8*4` or `ADD Rd, SP, #Word8*4`.
pub fn load_address(regs: &mut Registers, opcode: u16, instr_addr: u32) {
    let use_sp = (opcode >> 11) & 1 == 1;
    let rd = ((opcode >> 8) & 0x7) as usize;
    let word8 = (opcode & 0xFF) as u32;

    let base = if use_sp {
        regs.r(SP)
    } else {
        (instr_addr.wrapping_add(4)) & !2 // PC read-ahead, bit 1 forced to 0
    };
    regs.set_r(rd, base.wrapping_add(word8 * 4));
}

/// Format 13 — Add offset to stack pointer: `ADD/SUB SP, #SWord7*4`.
pub fn add_offset_to_sp(regs: &mut Registers, opcode: u16) {
    let negative = (opcode >> 7) & 1 == 1;
    let sword7 = (opcode & 0x7F) as u32;
    let offset = sword7 * 4;
    let sp = regs.r(SP);
    regs.set_r(SP, if negative { sp.wrapping_sub(offset) } else { sp.wrapping_add(offset) });
}

/// Format 14 — Push/pop registers: `PUSH {Rlist, LR?}` / `POP {Rlist, PC?}`.
/// A restricted STMDB/LDMIA fixed to SP, with an extra register (LR on
/// push, PC on pop) folded in via the R bit. Popping PC stays in Thumb
/// state and simply masks off bit 0 — unlike ARM's `LDM {..,PC}^`, a plain
/// Thumb POP never restores CPSR from SPSR.
pub fn push_pop<M: Memory>(regs: &mut Registers, mem: &mut M, opcode: u16) -> (u32, bool) {
    let pop = (opcode >> 11) & 1 == 1;
    let store_extra = (opcode >> 8) & 1 == 1;
    let list = (opcode & 0xFF) as u32;
    let registers: Vec<usize> = (0..8).filter(|&i| (list >> i) & 1 == 1).collect();
    let count = registers.len() as u32 + store_extra as u32;

    if pop {
        let mut addr = regs.r(SP);
        for &reg in &registers {
            regs.set_r(reg, mem.read32(addr));
            addr = addr.wrapping_add(4);
        }
        let mut pc_written = false;
        if store_extra {
            let value = mem.read32(addr);
            regs.set_r(PC, value & !1);
            addr = addr.wrapping_add(4);
            pc_written = true;
        }
        regs.set_r(SP, addr);
        (1 + count, pc_written)
    } else {
        let start = regs.r(SP).wrapping_sub(4 * count);
        let mut addr = start;
        for &reg in &registers {
            mem.write32(addr, regs.r(reg));
            addr = addr.wrapping_add(4);
        }
        if store_extra {
            mem.write32(addr, regs.r(LR));
        }
        regs.set_r(SP, start);
        (1 + count, false)
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
    fn sp_relative_store_then_load() {
        let mut bus = test_bus();
        let mut regs = Registers::reset();
        regs.set_r(SP, 0x0300_0000);
        regs.set_r(2, 0xABCD_1234);

        let str_op = (0b1001 << 12) | (0 << 11) | (2 << 8) | 4u16; // STR r2, [SP, #16]
        sp_relative(&mut regs, &mut bus, str_op);
        assert_eq!(bus.read32(0x0300_0010), 0xABCD_1234);

        let ldr_op = (0b1001 << 12) | (1 << 11) | (3 << 8) | 4u16; // LDR r3, [SP, #16]
        sp_relative(&mut regs, &mut bus, ldr_op);
        assert_eq!(regs.r(3), 0xABCD_1234);
    }

    #[test]
    fn load_address_from_pc_masks_bit1_and_scales() {
        let mut regs = Registers::reset();
        let op = (0b1010 << 12) | (0 << 11) | (0 << 8) | 1u16; // ADD r0, PC, #4
        load_address(&mut regs, op, 0x0000_1002);
        // instr_addr+4 = 0x1006, bit1 forced to 0 -> 0x1004, +4 -> 0x1008
        assert_eq!(regs.r(0), 0x0000_1008);
    }

    #[test]
    fn load_address_from_sp() {
        let mut regs = Registers::reset();
        regs.set_r(SP, 0x0300_0000);
        let op = (0b1010 << 12) | (1 << 11) | (1 << 8) | 2u16; // ADD r1, SP, #8
        load_address(&mut regs, op, 0);
        assert_eq!(regs.r(1), 0x0300_0008);
    }

    #[test]
    fn add_offset_to_sp_handles_sign() {
        let mut regs = Registers::reset();
        regs.set_r(SP, 0x0300_0100);
        add_offset_to_sp(&mut regs, (1 << 7) | 4u16); // SUB SP, #16
        assert_eq!(regs.r(SP), 0x0300_00F0);
        add_offset_to_sp(&mut regs, 4u16); // ADD SP, #16
        assert_eq!(regs.r(SP), 0x0300_0100);
    }

    #[test]
    fn push_then_pop_roundtrip_with_lr_and_pc() {
        let mut bus = test_bus();
        let mut regs = Registers::reset();
        regs.set_r(SP, 0x0300_0100);
        regs.set_r(0, 0x1111_1111);
        regs.set_r(1, 0x2222_2222);
        regs.set_r(LR, 0x0800_9999); // odd, as a real BL return address would be

        // PUSH {r0, r1, LR}
        let push_op = (0b1011 << 12) | (0 << 11) | (0b10 << 9) | (1 << 8) | 0b0000_0011u16;
        push_pop(&mut regs, &mut bus, push_op);
        assert_eq!(regs.r(SP), 0x0300_0100 - 12);

        regs.set_r(0, 0);
        regs.set_r(1, 0);
        // POP {r0, r1, PC}
        let pop_op = (0b1011 << 12) | (1 << 11) | (0b10 << 9) | (1 << 8) | 0b0000_0011u16;
        let (_, pc_written) = push_pop(&mut regs, &mut bus, pop_op);

        assert_eq!(regs.r(0), 0x1111_1111);
        assert_eq!(regs.r(1), 0x2222_2222);
        assert!(pc_written);
        assert_eq!(regs.pc(), 0x0800_9998, "POP {{..,PC}} masks off bit 0, unlike ARM's LDM+S restoring CPSR");
        assert_eq!(regs.r(SP), 0x0300_0100);
    }
}
