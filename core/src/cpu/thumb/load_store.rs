use crate::cpu::Registers;
use crate::memory::Memory;

/// Format 6 — PC-relative load: `LDR Rd, [PC, #Imm8*4]`. Per the Thumb
/// reference, PC here is read as instr_addr+4 with bit 1 forced to 0
/// (word-aligned), regardless of whether the instruction itself sits at a
/// word or halfword boundary.
pub fn pc_relative_load<M: Memory>(regs: &mut Registers, mem: &mut M, opcode: u16, instr_addr: u32) {
    let rd = ((opcode >> 8) & 0x7) as usize;
    let word8 = (opcode & 0xFF) as u32;
    let base = (instr_addr.wrapping_add(4)) & !3;
    let value = mem.read32(base.wrapping_add(word8 * 4));
    regs.set_r(rd, value);
}

/// Format 7 — Load/store with register offset: `STR/STRB/LDR/LDRB Rd, [Rb, Ro]`.
pub fn register_offset<M: Memory>(regs: &mut Registers, mem: &mut M, opcode: u16) {
    let load = (opcode >> 11) & 1 == 1;
    let byte = (opcode >> 10) & 1 == 1;
    let ro = ((opcode >> 6) & 0x7) as usize;
    let rb = ((opcode >> 3) & 0x7) as usize;
    let rd = (opcode & 0x7) as usize;
    let addr = regs.r(rb).wrapping_add(regs.r(ro));

    if load {
        let value = if byte {
            mem.read8(addr) as u32
        } else {
            mem.read32(addr & !3).rotate_right((addr & 3) * 8)
        };
        regs.set_r(rd, value);
    } else if byte {
        mem.write8(addr, regs.r(rd) as u8);
    } else {
        mem.write32(addr & !3, regs.r(rd));
    }
}

/// Format 8 — Load/store sign-extended byte/halfword:
/// `STRH/LDRH/LDSB/LDSH Rd, [Rb, Ro]`. Shares the same GBA misalignment
/// quirks as the ARM halfword-transfer group (see `arm::halfword_transfer`).
pub fn sign_extended<M: Memory>(regs: &mut Registers, mem: &mut M, opcode: u16) {
    let h = (opcode >> 11) & 1 == 1;
    let s = (opcode >> 10) & 1 == 1;
    let ro = ((opcode >> 6) & 0x7) as usize;
    let rb = ((opcode >> 3) & 0x7) as usize;
    let rd = (opcode & 0x7) as usize;
    let addr = regs.r(rb).wrapping_add(regs.r(ro));
    let misaligned = addr & 1 == 1;

    let value = match (s, h) {
        (false, false) => {
            mem.write16(addr & !1, regs.r(rd) as u16); // STRH
            return;
        }
        (false, true) => {
            if misaligned {
                mem.read16(addr & !1).rotate_right(8) as u32
            } else {
                mem.read16(addr) as u32
            } // LDRH
        }
        (true, false) => (mem.read8(addr) as i8) as i32 as u32, // LDSB
        (true, true) => {
            if misaligned {
                (mem.read8(addr) as i8) as i32 as u32 // GBA quirk: degrades to signed byte
            } else {
                (mem.read16(addr) as i16) as i32 as u32
            } // LDSH
        }
    };
    regs.set_r(rd, value);
}

/// Format 9 — Load/store with immediate offset: `STR/LDR/STRB/LDRB Rd,
/// [Rb, #Offset5]`. The immediate is scaled by 4 for word transfers, or
/// used directly for byte transfers.
pub fn immediate_offset<M: Memory>(regs: &mut Registers, mem: &mut M, opcode: u16) {
    let byte = (opcode >> 12) & 1 == 1;
    let load = (opcode >> 11) & 1 == 1;
    let offset5 = ((opcode >> 6) & 0x1F) as u32;
    let rb = ((opcode >> 3) & 0x7) as usize;
    let rd = (opcode & 0x7) as usize;
    let offset = if byte { offset5 } else { offset5 * 4 };
    let addr = regs.r(rb).wrapping_add(offset);

    if load {
        let value = if byte {
            mem.read8(addr) as u32
        } else {
            mem.read32(addr & !3).rotate_right((addr & 3) * 8)
        };
        regs.set_r(rd, value);
    } else if byte {
        mem.write8(addr, regs.r(rd) as u8);
    } else {
        mem.write32(addr & !3, regs.r(rd));
    }
}

/// Format 10 — Load/store halfword: `STRH/LDRH Rd, [Rb, #Offset5*2]`.
pub fn halfword<M: Memory>(regs: &mut Registers, mem: &mut M, opcode: u16) {
    let load = (opcode >> 11) & 1 == 1;
    let offset5 = ((opcode >> 6) & 0x1F) as u32;
    let rb = ((opcode >> 3) & 0x7) as usize;
    let rd = (opcode & 0x7) as usize;
    let addr = regs.r(rb).wrapping_add(offset5 * 2);

    if load {
        let value = if addr & 1 == 1 {
            mem.read16(addr & !1).rotate_right(8) as u32
        } else {
            mem.read16(addr) as u32
        };
        regs.set_r(rd, value);
    } else {
        mem.write16(addr & !1, regs.r(rd) as u16);
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
    fn pc_relative_load_word_aligns_pc_and_scales_offset() {
        let mut bus = test_bus();
        // IWRAM, not the 0x0000xxxx BIOS range — that's read-only, so a
        // write there would silently no-op and this test would read back 0.
        bus.write32(0x0300_0008, 0xCAFEBABE);
        let mut regs = Registers::reset();
        // LDR r0, [PC, #4]; instr at ...0002 (odd word-align case: +4 -> ...0006, &!3 -> ...0004, +4 -> ...0008)
        let op = (0b01001 << 11) | (0 << 8) | 1u16; // word8=1 -> offset 4
        pc_relative_load(&mut regs, &mut bus, op, 0x0300_0002);
        assert_eq!(regs.r(0), 0xCAFEBABE);
    }

    #[test]
    fn register_offset_ldr_and_str_roundtrip() {
        let mut bus = test_bus();
        let mut regs = Registers::reset();
        regs.set_r(1, 0x0300_0000); // Rb
        regs.set_r(2, 4); // Ro
        regs.set_r(3, 0x1234_5678); // value

        // STR r3, [r1, r2]
        let str_op = (0b0101 << 12) | (0 << 11) | (0 << 10) | (0 << 9) | (2 << 6) | (1 << 3) | 3;
        register_offset(&mut regs, &mut bus, str_op);
        assert_eq!(bus.read32(0x0300_0004), 0x1234_5678);

        // LDR r0, [r1, r2]
        let ldr_op = (0b0101 << 12) | (1 << 11) | (0 << 10) | (0 << 9) | (2 << 6) | (1 << 3) | 0;
        register_offset(&mut regs, &mut bus, ldr_op);
        assert_eq!(regs.r(0), 0x1234_5678);
    }

    #[test]
    fn sign_extended_ldsb_and_ldsh() {
        let mut bus = test_bus();
        bus.write8(0x0300_0000, 0x80);
        bus.write16(0x0300_0002, 0x8000);
        let mut regs = Registers::reset();
        regs.set_r(1, 0x0300_0000);
        regs.set_r(2, 0);

        let ldsb = (0b0101 << 12) | (0 << 11) | (1 << 10) | (1 << 9) | (2 << 6) | (1 << 3) | 0;
        sign_extended(&mut regs, &mut bus, ldsb);
        assert_eq!(regs.r(0), 0xFFFF_FF80);

        regs.set_r(2, 2);
        let ldsh = (0b0101 << 12) | (1 << 11) | (1 << 10) | (1 << 9) | (2 << 6) | (1 << 3) | 4;
        sign_extended(&mut regs, &mut bus, ldsh);
        assert_eq!(regs.r(4), 0xFFFF_8000);
    }

    #[test]
    fn immediate_offset_scales_by_4_for_words_and_not_for_bytes() {
        let mut bus = test_bus();
        bus.write32(0x0300_0008, 0xAABBCCDD);
        bus.write8(0x0300_001F, 0x11); // separate address: must not overlap the word above
        let mut regs = Registers::reset();
        regs.set_r(1, 0x0300_0000);

        // LDR r0, [r1, #8]  (offset5=2 -> 2*4=8)
        let ldr = (0b011 << 13) | (0 << 12) | (1 << 11) | (2 << 6) | (1 << 3) | 0;
        immediate_offset(&mut regs, &mut bus, ldr);
        assert_eq!(regs.r(0), 0xAABBCCDD);

        // LDRB r2, [r1, #31] (offset5=31, byte -> literal 31)
        let ldrb = (0b011 << 13) | (1 << 12) | (1 << 11) | (31 << 6) | (1 << 3) | 2;
        immediate_offset(&mut regs, &mut bus, ldrb);
        assert_eq!(regs.r(2), 0x11);
    }

    #[test]
    fn halfword_format_scales_by_2() {
        let mut bus = test_bus();
        let mut regs = Registers::reset();
        regs.set_r(1, 0x0300_0000);
        regs.set_r(2, 0xBEEF);

        // STRH r2, [r1, #6] (offset5=3 -> 3*2=6)
        let strh = (0b1000 << 12) | (0 << 11) | (3 << 6) | (1 << 3) | 2;
        halfword(&mut regs, &mut bus, strh);
        assert_eq!(bus.read16(0x0300_0006), 0xBEEF);

        let ldrh = (0b1000 << 12) | (1 << 11) | (3 << 6) | (1 << 3) | 4;
        halfword(&mut regs, &mut bus, ldrh);
        assert_eq!(regs.r(4), 0xBEEF);
    }
}
