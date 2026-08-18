use crate::cpu::Registers;
use crate::memory::Memory;

/// Format 15 — Multiple load/store: `STMIA/LDMIA Rb!, {Rlist}`, restricted
/// to low registers (r0-r7) for both the base and the list. Always
/// increment-after with writeback — there's no P/U/S bit like ARM's LDM/STM.
///
/// Scope note: if the base register is itself in the list, this always
/// stores/writes back using its pre-transfer value (the simple, common
/// interpretation); real hardware's behavior when the base isn't the first
/// list entry is a documented edge case this doesn't special-case, since
/// compiled code never relies on it.
pub fn execute<M: Memory>(regs: &mut Registers, mem: &mut M, opcode: u16) -> u32 {
    let load = (opcode >> 11) & 1 == 1;
    let rb = ((opcode >> 8) & 0x7) as usize;
    let list = (opcode & 0xFF) as u32;
    let registers: Vec<usize> = (0..8).filter(|&i| (list >> i) & 1 == 1).collect();
    let count = registers.len() as u32;

    let mut addr = regs.r(rb);
    for &reg in &registers {
        if load {
            regs.set_r(reg, mem.read32(addr & !3));
        } else {
            mem.write32(addr & !3, regs.r(reg));
        }
        addr = addr.wrapping_add(4);
    }

    if !(load && registers.contains(&rb)) {
        regs.set_r(rb, addr);
    }

    1 + count
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
    fn stmia_then_ldmia_roundtrip() {
        let mut bus = test_bus();
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0000);
        regs.set_r(1, 0xAAAA_AAAA);
        regs.set_r(2, 0xBBBB_BBBB);

        // STMIA r0!, {r1, r2}
        let stm = (0b1100 << 12) | (0 << 11) | (0 << 8) | 0b0000_0110u16;
        execute(&mut regs, &mut bus, stm);
        assert_eq!(regs.r(0), 0x0300_0008);
        assert_eq!(bus.read32(0x0300_0000), 0xAAAA_AAAA);
        assert_eq!(bus.read32(0x0300_0004), 0xBBBB_BBBB);

        regs.set_r(3, 0x0300_0000);
        // LDMIA r3!, {r4, r5}
        let ldm = (0b1100 << 12) | (1 << 11) | (3 << 8) | 0b0011_0000u16;
        execute(&mut regs, &mut bus, ldm);
        assert_eq!(regs.r(4), 0xAAAA_AAAA);
        assert_eq!(regs.r(5), 0xBBBB_BBBB);
        assert_eq!(regs.r(3), 0x0300_0008);
    }

    #[test]
    fn ldmia_with_base_in_list_keeps_the_loaded_value() {
        let mut bus = test_bus();
        bus.write32(0x0300_0000, 0xDEAD_BEEF);
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0000);

        let ldm = (0b1100 << 12) | (1 << 11) | (0 << 8) | 0b0000_0001u16; // LDMIA r0!, {r0}
        execute(&mut regs, &mut bus, ldm);
        assert_eq!(regs.r(0), 0xDEAD_BEEF);
    }
}
