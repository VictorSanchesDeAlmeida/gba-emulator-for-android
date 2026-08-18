use crate::cpu::Registers;
use crate::memory::Memory;

/// LDM/STM. Registers in the list are always transferred in ascending
/// register-number order, which is also always ascending memory-address
/// order — the P/U bits only decide where that block sits relative to the
/// base, not the iteration order.
///
/// Scope note: the S bit's "force user bank" behavior (used by OS code to
/// save/restore User-mode registers from a privileged mode) is not
/// implemented for non-PC transfers — regular game code never needs it.
/// The much more common S-bit use, restoring CPSR from SPSR when PC is in
/// the list (the "LDMFD SP!, {..,PC}^" exception-return idiom), is.
pub fn execute<M: Memory>(regs: &mut Registers, mem: &mut M, opcode: u32, instr_addr: u32) -> (u32, bool) {
    let pre_index = (opcode >> 24) & 1 == 1;
    let up = (opcode >> 23) & 1 == 1;
    let restore_cpsr_on_pc_load = (opcode >> 22) & 1 == 1;
    let writeback = (opcode >> 21) & 1 == 1;
    let load = (opcode >> 20) & 1 == 1;
    let rn = ((opcode >> 16) & 0xF) as usize;
    let list = opcode & 0xFFFF;

    let registers: Vec<usize> = (0..16).filter(|&i| (list >> i) & 1 == 1).collect();
    let count = registers.len() as u32;
    let base = regs.r(rn);

    let (start_address, rn_after) = match (pre_index, up) {
        (false, true) => (base, base.wrapping_add(4 * count)), // IA
        (true, true) => (base.wrapping_add(4), base.wrapping_add(4 * count)), // IB
        (false, false) => (base.wrapping_sub(4 * count).wrapping_add(4), base.wrapping_sub(4 * count)), // DA
        (true, false) => (base.wrapping_sub(4 * count), base.wrapping_sub(4 * count)), // DB
    };

    let mut addr = start_address;
    let mut pc_loaded = false;
    for &reg in &registers {
        if load {
            let value = mem.read32(addr & !3);
            regs.set_r(reg, value);
            if reg == 15 {
                pc_loaded = true;
            }
        } else {
            let value = if reg == 15 { instr_addr.wrapping_add(12) } else { regs.r(reg) };
            mem.write32(addr & !3, value);
        }
        addr = addr.wrapping_add(4);
    }

    if writeback && !(load && registers.contains(&rn)) {
        regs.set_r(rn, rn_after);
    }

    if pc_loaded && restore_cpsr_on_pc_load {
        let spsr = regs.spsr();
        regs.set_cpsr(spsr);
    }

    (1 + count, pc_loaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::{Cartridge, HEADER_SIZE};
    use crate::cpu::{psr, Mode};
    use crate::memory::Bus;

    fn test_bus() -> Bus {
        Bus::new(Cartridge::load(vec![0u8; HEADER_SIZE + 16]).unwrap())
    }

    fn encode(load: bool, pre: bool, up: bool, s: bool, writeback: bool, rn: u32, list: u32) -> u32 {
        (0xE << 28) | (0b100 << 25) | ((pre as u32) << 24) | ((up as u32) << 23) | ((s as u32) << 22)
            | ((writeback as u32) << 21) | ((load as u32) << 20) | (rn << 16) | list
    }

    #[test]
    fn stmia_then_ldmia_roundtrip() {
        let mut bus = test_bus();
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0000); // base
        regs.set_r(1, 0x1111_1111);
        regs.set_r(2, 0x2222_2222);
        regs.set_r(3, 0x3333_3333);

        // STMIA r0!, {r1-r3}
        let stm = encode(false, false, true, false, true, 0, 0b1110);
        execute(&mut regs, &mut bus, stm, 0);
        assert_eq!(regs.r(0), 0x0300_0000 + 12, "writeback advances by 4*count");
        assert_eq!(bus.read32(0x0300_0000), 0x1111_1111);
        assert_eq!(bus.read32(0x0300_0004), 0x2222_2222);
        assert_eq!(bus.read32(0x0300_0008), 0x3333_3333);

        // LDMIA r4!, {r5-r7} from the same block, via a fresh base register
        regs.set_r(4, 0x0300_0000);
        let ldm = encode(true, false, true, false, true, 4, 0b1110_0000);
        execute(&mut regs, &mut bus, ldm, 0);
        assert_eq!(regs.r(5), 0x1111_1111);
        assert_eq!(regs.r(6), 0x2222_2222);
        assert_eq!(regs.r(7), 0x3333_3333);
    }

    #[test]
    fn stmdb_full_descending_matches_ia_block_from_the_other_end() {
        let mut bus = test_bus();
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0010);
        regs.set_r(1, 0xAAAA_AAAA);
        regs.set_r(2, 0xBBBB_BBBB);

        // STMDB r0!, {r1,r2} (classic full-descending push)
        let op = encode(false, true, false, false, true, 0, 0b0110);
        execute(&mut regs, &mut bus, op, 0);

        assert_eq!(regs.r(0), 0x0300_0008);
        assert_eq!(bus.read32(0x0300_0008), 0xAAAA_AAAA, "lowest register at the lowest address");
        assert_eq!(bus.read32(0x0300_000C), 0xBBBB_BBBB);
    }

    #[test]
    fn ldm_with_pc_and_s_bit_restores_cpsr_from_spsr() {
        let mut bus = test_bus();
        bus.write32(0x0300_0000, 0x0800_9999); // value to load into PC
        let mut regs = Registers::reset();
        regs.set_mode(Mode::Irq);
        regs.set_spsr(Mode::User.bits());
        regs.set_r(13, 0x0300_0000);

        // LDMIA r13!, {r15}^  (S=1)
        let op = encode(true, false, true, true, true, 13, 0b1000_0000_0000_0000);
        let (_, pc_written) = execute(&mut regs, &mut bus, op, 0);

        assert!(pc_written);
        assert_eq!(regs.pc(), 0x0800_9999);
        assert_eq!(regs.mode(), Mode::User, "S-bit + PC in list restores CPSR from SPSR");
    }

    #[test]
    fn writeback_is_not_overwritten_when_base_register_is_in_the_load_list() {
        let mut bus = test_bus();
        bus.write32(0x0300_0000, 0xDEAD_BEEF);
        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0000);

        // LDMIA r0!, {r0} — the loaded value must win over writeback.
        let op = encode(true, false, true, false, true, 0, 0b0000_0001);
        execute(&mut regs, &mut bus, op, 0);
        assert_eq!(regs.r(0), 0xDEAD_BEEF);
    }

    #[test]
    fn stm_without_s_does_not_touch_flags() {
        let mut bus = test_bus();
        let mut regs = Registers::reset();
        regs.set_flag(psr::Z, true);
        regs.set_r(0, 0x0300_0000);
        regs.set_r(1, 1);
        let op = encode(false, false, true, false, false, 0, 0b0010);
        execute(&mut regs, &mut bus, op, 0);
        assert!(regs.flag(psr::Z));
    }
}
