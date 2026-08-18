use super::shifter::{self, ShiftType};
use crate::cpu::Registers;
use crate::memory::Memory;

/// LDR/STR/LDRB/STRB.
///
/// Two accuracy notes:
/// - An unaligned word `LDR` doesn't fault on real hardware — it reads the
///   word at the aligned address and rotates it right by `(addr & 3) * 8`.
///   `STR` simply forces the address to be aligned.
/// - Post-indexed transfers with W=1 request "unprivileged access" (LDRT/
///   STRT) on real hardware; this backend always writes back the base for
///   post-indexed transfers and doesn't force user-mode privilege, since
///   that distinction only matters for OS-level context switching.
pub fn execute<M: Memory>(regs: &mut Registers, mem: &mut M, opcode: u32, instr_addr: u32) -> (u32, bool) {
    let immediate_offset = (opcode >> 25) & 1 == 0;
    let pre_index = (opcode >> 24) & 1 == 1;
    let add = (opcode >> 23) & 1 == 1;
    let byte = (opcode >> 22) & 1 == 1;
    let writeback = (opcode >> 21) & 1 == 1;
    let load = (opcode >> 20) & 1 == 1;
    let rn = ((opcode >> 16) & 0xF) as usize;
    let rd = ((opcode >> 12) & 0xF) as usize;

    let offset = if immediate_offset {
        opcode & 0xFFF
    } else {
        let rm = (opcode & 0xF) as usize;
        let shift_type = ShiftType::from_bits(opcode >> 5);
        let amount = (opcode >> 7) & 0x1F;
        shifter::shift(shift_type, regs.r(rm), amount, false, true).value
    };

    let base = if rn == 15 { instr_addr.wrapping_add(8) } else { regs.r(rn) };
    let offset_addr = if add { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
    let effective_addr = if pre_index { offset_addr } else { base };

    if load {
        let value = if byte {
            mem.read8(effective_addr) as u32
        } else {
            let aligned = effective_addr & !3;
            mem.read32(aligned).rotate_right((effective_addr & 3) * 8)
        };
        regs.set_r(rd, value);
    } else {
        // ARM7TDMI-specific pipeline quirk: STR with Rd=r15 stores PC+12
        // (later ARM cores store PC+8).
        let value = if rd == 15 { instr_addr.wrapping_add(12) } else { regs.r(rd) };
        if byte {
            mem.write8(effective_addr, value as u8);
        } else {
            mem.write32(effective_addr & !3, value);
        }
    }

    if !pre_index || writeback {
        regs.set_r(rn, offset_addr);
    }

    (if load { 3 } else { 2 }, load && rd == 15)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::{Cartridge, HEADER_SIZE};
    use crate::memory::Bus;

    fn test_bus() -> Bus {
        Bus::new(Cartridge::load(vec![0u8; HEADER_SIZE + 16]).unwrap())
    }

    fn encode(load: bool, byte: bool, pre: bool, add: bool, writeback: bool, rn: u32, rd: u32, imm12: u32) -> u32 {
        (0xE << 28) | (0b01 << 26) | ((pre as u32) << 24) | ((add as u32) << 23) | ((byte as u32) << 22)
            | ((writeback as u32) << 21) | ((load as u32) << 20) | (rn << 16) | (rd << 12) | imm12
    }

    #[test]
    fn ldr_pre_indexed_with_writeback() {
        let mut bus = test_bus();
        bus.write32(0x0300_0010, 0xCAFEBABE);
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0000);

        let op = encode(true, false, true, true, true, 0, 1, 0x10);
        let (_, pc_written) = execute(&mut regs, &mut bus, op, 0);

        assert_eq!(regs.r(1), 0xCAFEBABE);
        assert_eq!(regs.r(0), 0x0300_0010, "base written back on pre-index with W=1");
        assert!(!pc_written);
    }

    #[test]
    fn str_post_indexed_always_writes_back() {
        let mut bus = test_bus();
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0000);
        regs.set_r(1, 0x1234_5678);

        let op = encode(false, false, false, true, false, 0, 1, 4);
        execute(&mut regs, &mut bus, op, 0);

        assert_eq!(bus.read32(0x0300_0000), 0x1234_5678, "store uses the base, not base+offset");
        assert_eq!(regs.r(0), 0x0300_0004, "post-index always writes back");
    }

    #[test]
    fn ldrb_zero_extends() {
        let mut bus = test_bus();
        bus.write8(0x0300_0000, 0xFF);
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0000);

        let op = encode(true, true, true, true, false, 0, 1, 0);
        execute(&mut regs, &mut bus, op, 0);
        assert_eq!(regs.r(1), 0xFF, "byte load must not sign-extend");
    }

    #[test]
    fn unaligned_ldr_word_rotates() {
        let mut bus = test_bus();
        bus.write32(0x0300_0000, 0x1122_3344);
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0001); // unaligned by 1

        let op = encode(true, false, true, true, false, 0, 1, 0);
        execute(&mut regs, &mut bus, op, 0);
        assert_eq!(regs.r(1), 0x1122_3344u32.rotate_right(8));
    }

    #[test]
    fn str_pc_stores_instruction_address_plus_12() {
        let mut bus = test_bus();
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0000);

        let op = encode(false, false, true, true, false, 0, 15, 0);
        execute(&mut regs, &mut bus, op, 0x0800_1000);
        assert_eq!(bus.read32(0x0300_0000), 0x0800_1000 + 12);
    }
}
