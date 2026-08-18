use crate::cpu::Registers;
use crate::memory::Memory;

/// LDRH/STRH/LDRSB/LDRSH — the halfword and signed-byte/halfword transfer
/// group (a separate encoding from LDR/STR, sharing bits 27-25==000 with
/// Data Processing/Multiply but distinguished by bits 7-4).
///
/// GBA-specific misalignment quirks (see GBATEK): an unaligned `LDRH`
/// rotates the aligned halfword right by 8; an unaligned `LDRSH` instead
/// degrades to a signed *byte* read at that address, rather than rotating.
pub fn execute<M: Memory>(regs: &mut Registers, mem: &mut M, opcode: u32, instr_addr: u32) -> (u32, bool) {
    let pre_index = (opcode >> 24) & 1 == 1;
    let add = (opcode >> 23) & 1 == 1;
    let immediate_offset = (opcode >> 22) & 1 == 1;
    let writeback = (opcode >> 21) & 1 == 1;
    let load = (opcode >> 20) & 1 == 1;
    let rn = ((opcode >> 16) & 0xF) as usize;
    let rd = ((opcode >> 12) & 0xF) as usize;
    let signed = (opcode >> 6) & 1 == 1;
    let half = (opcode >> 5) & 1 == 1;

    let offset = if immediate_offset {
        (((opcode >> 8) & 0xF) << 4) | (opcode & 0xF)
    } else {
        let rm = (opcode & 0xF) as usize;
        regs.r(rm)
    };

    let base = if rn == 15 { instr_addr.wrapping_add(8) } else { regs.r(rn) };
    let offset_addr = if add { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
    let effective_addr = if pre_index { offset_addr } else { base };
    let misaligned = effective_addr & 1 == 1;

    if load {
        let value = match (signed, half) {
            (false, true) => {
                if misaligned {
                    mem.read16(effective_addr & !1).rotate_right(8) as u32
                } else {
                    mem.read16(effective_addr) as u32
                }
            }
            (true, false) => (mem.read8(effective_addr) as i8) as i32 as u32,
            (true, true) => {
                if misaligned {
                    (mem.read8(effective_addr) as i8) as i32 as u32 // GBA quirk: degrades to LDRSB
                } else {
                    (mem.read16(effective_addr) as i16) as i32 as u32
                }
            }
            (false, false) => unreachable!("S=0,H=0 belongs to the Multiply/Swap encoding space"),
        };
        regs.set_r(rd, value);
    } else {
        mem.write16(effective_addr & !1, regs.r(rd) as u16);
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

    fn encode(load: bool, signed: bool, half: bool, pre: bool, add: bool, imm_offset: bool, writeback: bool, rn: u32, rd: u32, offset: u32) -> u32 {
        let (hi, lo) = ((offset >> 4) & 0xF, offset & 0xF);
        (0xE << 28) | ((pre as u32) << 24) | ((add as u32) << 23) | ((imm_offset as u32) << 22)
            | ((writeback as u32) << 21) | ((load as u32) << 20) | (rn << 16) | (rd << 12)
            | (hi << 8) | (1 << 7) | ((signed as u32) << 6) | ((half as u32) << 5) | (1 << 4) | lo
    }

    #[test]
    fn ldrh_and_strh_roundtrip() {
        let mut bus = test_bus();
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0000);
        regs.set_r(1, 0xBEEF);

        let strh = encode(false, false, true, true, true, true, false, 0, 1, 0);
        execute(&mut regs, &mut bus, strh, 0);
        assert_eq!(bus.read16(0x0300_0000), 0xBEEF);

        let ldrh = encode(true, false, true, true, true, true, false, 0, 2, 0);
        execute(&mut regs, &mut bus, ldrh, 0);
        assert_eq!(regs.r(2), 0xBEEF);
    }

    #[test]
    fn ldrsb_sign_extends() {
        let mut bus = test_bus();
        bus.write8(0x0300_0000, 0x80); // -128 as i8
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0000);

        let op = encode(true, true, false, true, true, true, false, 0, 1, 0);
        execute(&mut regs, &mut bus, op, 0);
        assert_eq!(regs.r(1), 0xFFFF_FF80);
    }

    #[test]
    fn ldrsh_sign_extends_when_aligned() {
        let mut bus = test_bus();
        bus.write16(0x0300_0000, 0x8000); // negative i16
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0000);

        let op = encode(true, true, true, true, true, true, false, 0, 1, 0);
        execute(&mut regs, &mut bus, op, 0);
        assert_eq!(regs.r(1), 0xFFFF_8000);
    }

    #[test]
    fn ldrsh_misaligned_degrades_to_signed_byte_read() {
        let mut bus = test_bus();
        bus.write8(0x0300_0001, 0x80); // the byte at the odd address
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0001);

        let op = encode(true, true, true, true, true, true, false, 0, 1, 0);
        execute(&mut regs, &mut bus, op, 0);
        assert_eq!(regs.r(1), 0xFFFF_FF80, "GBA quirk: misaligned LDRSH reads a signed byte");
    }

    #[test]
    fn ldrh_misaligned_rotates() {
        let mut bus = test_bus();
        bus.write16(0x0300_0000, 0x1234);
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0001);

        let op = encode(true, false, true, true, true, true, false, 0, 1, 0);
        execute(&mut regs, &mut bus, op, 0);
        // The rotate happens within the 16-bit halfword (byte swap), which
        // is what brings the actually-addressed byte to the LSB — same
        // principle as LDR's word rotate — before zero-extending to 32 bits.
        assert_eq!(regs.r(1), 0x1234u16.rotate_right(8) as u32);
    }
}
