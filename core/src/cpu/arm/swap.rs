use crate::cpu::Registers;
use crate::memory::Memory;

/// SWP/SWPB: reads `[Rn]`, writes `Rm` to `[Rn]`, then puts the value that
/// was read into `Rd`. Modeled as read-then-write in program order, which
/// is all that matters for a single-threaded interpreter (the real
/// instruction's atomicity guarantee only matters with multiple bus
/// masters, which the GBA doesn't have).
pub fn execute<M: Memory>(regs: &mut Registers, mem: &mut M, opcode: u32) -> (u32, bool) {
    let byte = (opcode >> 22) & 1 == 1;
    let rn = ((opcode >> 16) & 0xF) as usize;
    let rd = ((opcode >> 12) & 0xF) as usize;
    let rm = (opcode & 0xF) as usize;

    let addr = regs.r(rn);
    if byte {
        let old = mem.read8(addr);
        mem.write8(addr, regs.r(rm) as u8);
        regs.set_r(rd, old as u32);
    } else {
        let aligned = addr & !3;
        let old = mem.read32(aligned).rotate_right((addr & 3) * 8);
        mem.write32(aligned, regs.r(rm));
        regs.set_r(rd, old);
    }
    (2, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::{Cartridge, HEADER_SIZE};
    use crate::memory::Bus;

    fn test_bus() -> Bus {
        Bus::new(Cartridge::load(vec![0u8; HEADER_SIZE + 16]).unwrap())
    }

    fn encode_swp(byte: bool, rn: u32, rd: u32, rm: u32) -> u32 {
        (0xE << 28) | (1 << 24) | ((byte as u32) << 22) | (rn << 16) | (rd << 12) | (0b1001 << 4) | rm
    }

    #[test]
    fn swaps_a_word() {
        let mut bus = test_bus();
        bus.write32(0x0300_0000, 0xAAAA_BBBB);
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0000); // Rn: address
        regs.set_r(1, 0x1111_2222); // Rm: new value

        let op = encode_swp(false, 0, 2, 1); // SWP r2, r1, [r0]
        execute(&mut regs, &mut bus, op);

        assert_eq!(regs.r(2), 0xAAAA_BBBB, "Rd gets the old memory value");
        assert_eq!(bus.read32(0x0300_0000), 0x1111_2222, "memory gets Rm");
    }

    #[test]
    fn swaps_a_byte() {
        let mut bus = test_bus();
        bus.write8(0x0300_0000, 0x42);
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0000);
        regs.set_r(1, 0x99);

        let op = encode_swp(true, 0, 2, 1);
        execute(&mut regs, &mut bus, op);

        assert_eq!(regs.r(2), 0x42);
        assert_eq!(bus.read8(0x0300_0000), 0x99);
    }
}
