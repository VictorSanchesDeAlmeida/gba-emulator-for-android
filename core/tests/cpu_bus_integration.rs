//! Integration test: runs the CPU against the real `Bus` (not a fake), by
//! hand-assembling a tiny ARM program directly into IWRAM and stepping the
//! CPU through it. Exercises the `Memory` trait wiring end to end, not just
//! the ARM decode logic already covered by unit tests.

use gba_core::cartridge::{Cartridge, HEADER_SIZE};
use gba_core::cpu::Cpu;
use gba_core::memory::Bus;

fn minimal_bus() -> Bus {
    let rom = vec![0u8; HEADER_SIZE + 16];
    Bus::new(Cartridge::load(rom).unwrap())
}

fn encode_dp(cond: u32, opcode4: u32, s: u32, rn: u32, rd: u32, operand2: u32, immediate: bool) -> u32 {
    (cond << 28) | ((immediate as u32) << 25) | (opcode4 << 21) | (s << 20) | (rn << 16) | (rd << 12) | operand2
}

#[test]
fn runs_mov_add_and_a_loop_branch_through_the_real_bus() {
    let mut bus = minimal_bus();
    let mut cpu = Cpu::new();

    let base = 0x0300_0000u32; // IWRAM
    // MOV r0, #5
    bus.write32(base, encode_dp(0xE, 0xD, 0, 0, 0, 5, true));
    // MOV r1, #10
    bus.write32(base + 4, encode_dp(0xE, 0xD, 0, 0, 1, 10, true));
    // ADDS r2, r0, r1
    bus.write32(base + 8, encode_dp(0xE, 0x4, 1, 0, 2, 1, false));
    // B . (branch to self, offset -2 words = back to this same instruction)
    bus.write32(base + 12, 0xEAFF_FFFE);

    cpu.registers.set_pc(base);

    cpu.step(&mut bus).unwrap(); // MOV r0, #5
    cpu.step(&mut bus).unwrap(); // MOV r1, #10
    cpu.step(&mut bus).unwrap(); // ADDS r2, r0, r1

    assert_eq!(cpu.registers.r(0), 5);
    assert_eq!(cpu.registers.r(1), 10);
    assert_eq!(cpu.registers.r(2), 15);
    assert_eq!(cpu.registers.pc(), base + 12);

    cpu.step(&mut bus).unwrap(); // B . (infinite loop back to itself)
    assert_eq!(cpu.registers.pc(), base + 12, "branch offset -2 targets its own address");
}

#[test]
fn str_ldr_roundtrip_then_bx_into_thumb_and_keep_executing() {
    let mut bus = minimal_bus();
    let mut cpu = Cpu::new();

    let base = 0x0300_0000u32;
    let scratch = 0x0300_0040u32;
    let thumb_target = 0x0300_1000u32;
    cpu.registers.set_r(0, 0x42); // value to store
    cpu.registers.set_r(1, scratch); // address
    cpu.registers.set_r(3, thumb_target | 1); // BX target: odd address selects Thumb state

    // STR r0, [r1]
    bus.write32(base, 0xE581_0000);
    // LDR r2, [r1]
    bus.write32(base + 4, 0xE591_2000);
    // BX r3
    bus.write32(base + 8, 0xE12F_FF13);

    cpu.registers.set_pc(base);
    cpu.step(&mut bus).unwrap(); // STR
    cpu.step(&mut bus).unwrap(); // LDR
    assert_eq!(cpu.registers.r(2), 0x42, "value survives a store/load roundtrip through the real bus");

    cpu.step(&mut bus).unwrap(); // BX r3
    assert!(cpu.registers.in_thumb_state());
    assert_eq!(cpu.registers.pc(), thumb_target);

    // Thumb code now actually runs: MOV r5, #7 (Format 3).
    bus.write16(thumb_target, 0x2507);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.registers.r(5), 7, "Thumb decoding executes real instructions through the bus");
    assert_eq!(cpu.registers.pc(), thumb_target + 2);
}
