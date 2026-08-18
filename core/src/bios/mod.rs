//! High-level emulation (HLE) of the GBA BIOS.
//!
//! This project doesn't ship — and can't legally ship — Nintendo's BIOS
//! ROM. Without one, real commercial games are unplayable: every game's
//! startup code calls BIOS functions via `SWI` (memory clears, `Div`,
//! `CpuSet`, decompression, and above all `IntrWait`/`VBlankIntrWait` for
//! frame pacing) within its first few dozen instructions, and every
//! enabled hardware interrupt vectors through BIOS code at `0x00000018`
//! to acknowledge itself. With no BIOS there, either of those strands the
//! CPU executing all-zero memory forever, having decoded byte pattern
//! `0x00000000` as a harmless-looking (but never-returning) instruction.
//!
//! Instead of replicating that missing machine code, [`Emulator::step`]
//! intercepts both situations directly:
//! - An `SWI` instruction about to execute is recognized before it runs;
//!   [`try_handle_swi`] performs that function's *documented effect* on
//!   registers/memory in Rust, then advances PC to the instruction after
//!   the `SWI` — exactly the end-to-end effect a real BIOS call has,
//!   without modeling the CPU mode round-trip that produces it.
//! - PC landing on the IRQ vector in IRQ mode (i.e. `Cpu::enter_irq` just
//!   ran) is recognized before any instruction there executes;
//!   [`service_interrupt`] acknowledges the firing IE&IF bits, ORs them
//!   into the fixed RAM mirror at [`INTR_CHECK_ADDR`] that `IntrWait`/
//!   `VBlankIntrWait` poll, then — matching real BIOS behavior — checks
//!   the game-installed handler pointer at `0x03007FFC`. If one is
//!   installed, it's actually *executed* as real code (the interrupted
//!   return address is pushed to the IRQ stack first, `LR` is pointed at
//!   [`IRQ_RETURN_STUB`], and PC jumps to the handler); when the handler
//!   eventually returns via its own `BX LR`, landing on that stub is
//!   recognized the same way and [`finish_interrupt_after_handler`] pops
//!   the stack and completes the return from interrupt. If no handler is
//!   installed, the interrupt returns immediately instead. Many games
//!   only need the mirror update (that's all `VBlankIntrWait`-style
//!   pacing polls) but some — this project's original real-ROM test
//!   fixture among them — poll state their *own* handler updates
//!   directly, and hang forever without this.

use crate::cpu::{psr, Registers};
use crate::memory::Bus;

/// Where [`crate::cpu::Cpu::enter_irq`] jumps to — see the module doc for
/// why nothing real needs to live there.
pub(crate) const IRQ_VECTOR: u32 = 0x0000_0018;

/// An otherwise-never-executed BIOS address used as the synthetic return
/// point from a game-installed interrupt handler — see the module doc.
pub(crate) const IRQ_RETURN_STUB: u32 = 0x0000_01FC;

/// Fixed IWRAM address holding the pointer to a game-installed interrupt
/// handler, if any — `0` means none installed.
const USER_HANDLER_PTR_ADDR: u32 = 0x0300_7FFC;

/// Fixed IWRAM address the real BIOS's `IntrWait`/`VBlankIntrWait`
/// (and this module's HLE of them) poll: every newly-acknowledged
/// interrupt's bit gets OR'd in here, and a satisfied wait clears its bits
/// back out.
pub(crate) const INTR_CHECK_ADDR: u32 = 0x0300_7FF8;

/// What [`try_handle_swi`] found and did.
pub(crate) enum SwiOutcome {
    /// Handled entirely here; PC has already been advanced past the SWI.
    /// The `u16` is a halt-wait mask to install (`Halt`/`IntrWait`/
    /// `VBlankIntrWait`) — `None` for functions that don't wait.
    Handled { halt_mask: Option<u16> },
    /// `SoftReset` (SWI 0x00): the caller should re-run the same boot
    /// sequence used for a fresh `Emulator`, since that's a superset of
    /// what this function needs (stack pointers, mode, entry PC).
    SoftReset,
}

/// If `regs.pc()` currently holds an `SWI` instruction (ARM or Thumb),
/// performs its HLE effect and returns `Some`. Every recognized SWI
/// number is intercepted — including ones this module doesn't implement,
/// which become a harmless no-op-and-continue rather than falling through
/// to the real (nonexistent) BIOS exception. See the module doc.
pub(crate) fn try_handle_swi(regs: &mut Registers, bus: &mut Bus) -> Option<SwiOutcome> {
    let (number, next_pc) = if regs.in_thumb_state() {
        let pc = regs.pc() & !1;
        let opcode = bus.read16(pc);
        if opcode & 0xFF00 != 0xDF00 {
            return None;
        }
        (opcode & 0xFF, pc.wrapping_add(2))
    } else {
        let pc = regs.pc() & !3;
        let opcode = bus.read32(pc);
        if opcode & 0x0F00_0000 != 0x0F00_0000 {
            return None;
        }
        (((opcode >> 16) & 0xFF) as u16, pc.wrapping_add(4))
    };

    if number == 0x00 {
        return Some(SwiOutcome::SoftReset);
    }

    let mut halt_mask = None;
    match number {
        0x01 => register_ram_reset(regs, bus),
        0x02 => halt_mask = Some(0xFFFF), // Halt: wake on any enabled+pending interrupt
        0x04 => {
            // IntrWait(clear_old, wait_flags)
            let mask = regs.r(1) as u16;
            if regs.r(0) != 0 {
                let mirror = bus.read16(INTR_CHECK_ADDR);
                bus.write16(INTR_CHECK_ADDR, mirror & !mask);
            }
            halt_mask = Some(mask);
        }
        0x05 => {
            // VBlankIntrWait: always clears-then-waits for VBlank specifically.
            let mirror = bus.read16(INTR_CHECK_ADDR);
            bus.write16(INTR_CHECK_ADDR, mirror & !0x0001);
            halt_mask = Some(0x0001);
        }
        0x06 => div(regs, false),
        0x07 => div(regs, true),
        0x08 => sqrt(regs),
        0x0B => cpu_set(regs, bus),
        0x0C => cpu_fast_set(regs, bus),
        0x11 => lz77_decompress(bus, regs.r(0), regs.r(1)),
        0x12 => lz77_decompress(bus, regs.r(0), regs.r(1)),
        _ => {} // unimplemented function: treated as a no-op, not a hang
    }

    regs.set_pc(next_pc);
    Some(SwiOutcome::Handled { halt_mask })
}

/// Acknowledges every IE&IF-matching bit and ORs them into the `IntrWait`
/// RAM mirror, then either calls the game's installed interrupt handler
/// (if any) or returns from the interrupt immediately — see the module
/// doc for the two-phase design this requires.
pub(crate) fn service_interrupt(regs: &mut Registers, bus: &mut Bus) {
    let acked = bus.interrupts().ie_and_iflag();
    bus.interrupts_mut().acknowledge(acked);
    let mirror = bus.read16(INTR_CHECK_ADDR);
    bus.write16(INTR_CHECK_ADDR, mirror | acked);

    // LR_irq must be read *before* restoring CPSR: the mode switch that
    // triggers banks r13/r14 back to whatever mode SPSR_irq holds, so
    // reading r14 afterward would silently grab that other mode's LR.
    let return_addr = regs.r(14).wrapping_sub(4);

    let handler = bus.read32(USER_HANDLER_PTR_ADDR);
    if handler == 0 {
        regs.set_cpsr(regs.spsr());
        regs.set_pc(return_addr);
        return;
    }

    // A handler is installed: call it for real, the way the BIOS's own
    // stub does — push the true return address to the IRQ stack (SPSR_irq
    // is safe in its own bank, but LR_irq itself would otherwise be
    // clobbered the instant the handler makes its own function calls),
    // point LR at our synthetic return stub, and jump in.
    let sp = regs.r(13).wrapping_sub(4);
    regs.set_r(13, sp);
    bus.write32(sp, return_addr);
    regs.set_r(14, IRQ_RETURN_STUB);
    regs.set_flag(psr::T, handler & 1 != 0);
    regs.set_pc(handler & !1);
}

/// Completes an interrupt whose game-installed handler just returned
/// (`PC == IRQ_RETURN_STUB`): pops the true return address the handler's
/// `BX LR` bypassed, and returns from the interrupt.
pub(crate) fn finish_interrupt_after_handler(regs: &mut Registers, bus: &mut Bus) {
    let sp = regs.r(13);
    let return_addr = bus.read32(sp);
    regs.set_r(13, sp.wrapping_add(4));
    regs.set_cpsr(regs.spsr());
    regs.set_pc(return_addr);
}

/// Whether a halted CPU's wait condition (`mask`) is now satisfied by the
/// RAM mirror, consuming (clearing) the matching bits if so — mirrors
/// real `IntrWait`'s own "wait, then consume" behavior.
/// Checks the *raw* IE&IF bits, not the RAM mirror — matching a real
/// documented GBA quirk: `Halt` (and therefore `IntrWait`/
/// `VBlankIntrWait`, which loop on it) wakes purely on IE&IF, completely
/// independent of IME. IME only gates whether the CPU goes on to actually
/// *service* the interrupt (jump to the vector) — a game that has
/// configured DISPSTAT/IE for VBlank but hasn't set IME yet (very common
/// early in a game's boot sequence, before its first `VBlankIntrWait`
/// call) must still wake up here, exactly like real hardware. When it
/// fires, this acknowledges the matching bits and updates the mirror
/// itself, standing in for the servicing that would normally do that.
pub(crate) fn check_halt_wakeup(bus: &mut Bus, mask: u16) -> bool {
    let fired = bus.interrupts().ie_and_iflag() & mask;
    if fired == 0 {
        return false;
    }
    bus.interrupts_mut().acknowledge(fired);
    let mirror = bus.read16(INTR_CHECK_ADDR);
    bus.write16(INTR_CHECK_ADDR, mirror | fired);
    true
}

fn register_ram_reset(regs: &Registers, bus: &mut Bus) {
    let flags = regs.r(0);
    if flags & 0x01 != 0 {
        bus.clear_ewram();
    }
    if flags & 0x02 != 0 {
        bus.clear_iwram_except_top();
    }
    if flags & 0x04 != 0 {
        bus.ppu_mut().palette_mut().fill(0);
    }
    if flags & 0x08 != 0 {
        bus.ppu_mut().vram_mut().fill(0);
    }
    if flags & 0x10 != 0 {
        bus.ppu_mut().oam_mut().fill(0);
    }
}

/// `Div`/`DivArm` (SWI 0x06/0x07): `swapped` selects DivArm's historical
/// numerator/denominator-swapped argument order. r0=quotient,
/// r1=remainder, r3=|quotient|.
fn div(regs: &mut Registers, swapped: bool) {
    let (num, denom) =
        if swapped { (regs.r(1) as i32, regs.r(0) as i32) } else { (regs.r(0) as i32, regs.r(1) as i32) };
    if denom == 0 {
        // Real hardware hangs/misbehaves on divide-by-zero; there's no
        // sane "correct" value, so this just avoids a Rust panic.
        regs.set_r(0, 0);
        regs.set_r(1, num as u32);
        regs.set_r(3, 0);
        return;
    }
    let quotient = num / denom;
    regs.set_r(0, quotient as u32);
    regs.set_r(1, (num % denom) as u32);
    regs.set_r(3, quotient.unsigned_abs());
}

/// `Sqrt` (SWI 0x08): r0 = integer square root of unsigned r0.
fn sqrt(regs: &mut Registers) {
    let value = regs.r(0);
    regs.set_r(0, (value as f64).sqrt() as u32);
}

/// `CpuSet` (SWI 0x0B): word/halfword block copy or fill. r0=source,
/// r1=dest, r2=control (bits0-20=count, bit24=fixed source, bit26=32-bit
/// transfer else 16-bit).
fn cpu_set(regs: &Registers, bus: &mut Bus) {
    let control = regs.r(2);
    let count = control & 0x1F_FFFF;
    let fixed_src = control & 0x0100_0000 != 0;
    let word32 = control & 0x0400_0000 != 0;

    let mut src = regs.r(0);
    let mut dst = regs.r(1);
    for _ in 0..count {
        if word32 {
            bus.write32(dst, bus.read32(src));
            dst = dst.wrapping_add(4);
            if !fixed_src {
                src = src.wrapping_add(4);
            }
        } else {
            bus.write16(dst, bus.read16(src));
            dst = dst.wrapping_add(2);
            if !fixed_src {
                src = src.wrapping_add(2);
            }
        }
    }
}

/// `CpuFastSet` (SWI 0x0C): like `CpuSet` but always 32-bit. Real hardware
/// requires `count` to be a multiple of 8 words; this just processes
/// exactly `count` words regardless, which is equivalent for any
/// correctly-formed call.
fn cpu_fast_set(regs: &Registers, bus: &mut Bus) {
    let control = regs.r(2);
    let count = control & 0x1F_FFFF;
    let fixed_src = control & 0x0100_0000 != 0;

    let mut src = regs.r(0);
    let mut dst = regs.r(1);
    for _ in 0..count {
        bus.write32(dst, bus.read32(src));
        dst = dst.wrapping_add(4);
        if !fixed_src {
            src = src.wrapping_add(4);
        }
    }
}

/// `LZ77UnCompWram`/`LZ77UnCompVram` (SWI 0x11/0x12): decompresses an
/// LZ77-compressed block. Both variants are implemented identically here
/// (the WRAM/VRAM distinction on real hardware is about access width/
/// alignment restrictions this emulator doesn't need to model).
fn lz77_decompress(bus: &mut Bus, source: u32, dest: u32) {
    let header = bus.read32(source);
    if header & 0xFF != 0x10 {
        return; // not actually LZ77-tagged data; nothing sane to do
    }
    let size = header >> 8;

    let mut out: Vec<u8> = Vec::with_capacity(size as usize);
    let mut src = source + 4;
    while (out.len() as u32) < size {
        let flags = bus.read8(src);
        src += 1;
        for bit in (0..8).rev() {
            if out.len() as u32 >= size {
                break;
            }
            if flags & (1 << bit) == 0 {
                out.push(bus.read8(src));
                src += 1;
            } else {
                let b0 = bus.read8(src) as u32;
                let b1 = bus.read8(src + 1) as u32;
                src += 2;
                let length = (b0 >> 4) + 3;
                let disp = ((b0 & 0xF) << 8 | b1) + 1;
                for _ in 0..length {
                    if out.len() as u32 >= size {
                        break;
                    }
                    let byte = out[out.len() - disp as usize];
                    out.push(byte);
                }
            }
        }
    }

    for (i, &byte) in out.iter().enumerate() {
        bus.write8(dest + i as u32, byte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::{Cartridge, HEADER_SIZE};

    fn test_bus() -> Bus {
        let rom = vec![0u8; HEADER_SIZE + 16];
        Bus::new(Cartridge::load(rom).unwrap())
    }

    #[test]
    fn div_computes_quotient_remainder_and_abs() {
        let mut regs = Registers::reset();
        regs.set_r(0, (-7i32) as u32);
        regs.set_r(1, 2);
        div(&mut regs, false);
        assert_eq!(regs.r(0) as i32, -3);
        assert_eq!(regs.r(1) as i32, -1);
        assert_eq!(regs.r(3), 3);
    }

    #[test]
    fn div_arm_swaps_operand_order() {
        let mut regs = Registers::reset();
        regs.set_r(0, 2); // denominator in the swapped convention
        regs.set_r(1, 10); // numerator
        div(&mut regs, true);
        assert_eq!(regs.r(0), 5);
    }

    #[test]
    fn div_by_zero_does_not_panic() {
        let mut regs = Registers::reset();
        regs.set_r(0, 5);
        regs.set_r(1, 0);
        div(&mut regs, false);
    }

    #[test]
    fn sqrt_computes_integer_root() {
        let mut regs = Registers::reset();
        regs.set_r(0, 100);
        sqrt(&mut regs);
        assert_eq!(regs.r(0), 10);
    }

    #[test]
    fn cpu_set_copies_words() {
        let mut bus = test_bus();
        bus.write32(0x0300_0000, 0xAABBCCDD);
        bus.write32(0x0300_0004, 0x11223344);

        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0000);
        regs.set_r(1, 0x0300_1000);
        regs.set_r(2, 2 | 0x0400_0000); // count=2, 32-bit
        cpu_set(&regs, &mut bus);

        assert_eq!(bus.read32(0x0300_1000), 0xAABBCCDD);
        assert_eq!(bus.read32(0x0300_1004), 0x11223344);
    }

    #[test]
    fn cpu_set_fixed_source_repeats_the_same_word() {
        let mut bus = test_bus();
        bus.write32(0x0300_0000, 0x5555_5555);

        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0000);
        regs.set_r(1, 0x0300_1000);
        regs.set_r(2, 3 | 0x0100_0000 | 0x0400_0000); // count=3, fixed source, 32-bit
        cpu_set(&regs, &mut bus);

        assert_eq!(bus.read32(0x0300_1000), 0x5555_5555);
        assert_eq!(bus.read32(0x0300_1004), 0x5555_5555);
        assert_eq!(bus.read32(0x0300_1008), 0x5555_5555);
    }

    #[test]
    fn cpu_fast_set_copies_words() {
        let mut bus = test_bus();
        bus.write32(0x0300_0000, 0x1234_5678);

        let mut regs = Registers::reset();
        regs.set_r(0, 0x0300_0000);
        regs.set_r(1, 0x0300_1000);
        regs.set_r(2, 1);
        cpu_fast_set(&regs, &mut bus);

        assert_eq!(bus.read32(0x0300_1000), 0x1234_5678);
    }

    #[test]
    fn register_ram_reset_clears_only_the_requested_regions() {
        let mut bus = test_bus();
        bus.write8(0x0200_0000, 0xFF); // EWRAM
        bus.write8(0x0300_0000, 0xFF); // IWRAM
        bus.ppu_mut().palette_mut()[0] = 0xFF;

        let mut regs = Registers::reset();
        regs.set_r(0, 0x01); // EWRAM only
        register_ram_reset(&regs, &mut bus);

        assert_eq!(bus.read8(0x0200_0000), 0);
        assert_eq!(bus.read8(0x0300_0000), 0xFF, "IWRAM flag wasn't set");
        assert_eq!(bus.ppu().palette()[0], 0xFF, "palette flag wasn't set");
    }

    #[test]
    fn register_ram_reset_preserves_the_top_of_iwram() {
        let mut bus = test_bus();
        let preserved_addr = 0x0300_0000 + 0x8000 - 0x100; // within the preserved top 0x200
        bus.write8(preserved_addr, 0xAB);
        bus.write8(0x0300_0000, 0xFF); // well below the preserved area

        let mut regs = Registers::reset();
        regs.set_r(0, 0x02); // IWRAM
        register_ram_reset(&regs, &mut bus);

        assert_eq!(bus.read8(0x0300_0000), 0, "the bulk of IWRAM must be cleared");
        assert_eq!(bus.read8(preserved_addr), 0xAB, "the top 0x200 bytes must survive a reset");
    }

    #[test]
    fn lz77_decompress_expands_literals_and_back_references() {
        let mut bus = test_bus();
        // Source lives in IWRAM (ROM is read-only on the bus). Header:
        // type 0x10, decompressed size = 6. Stream: 3 literal bytes "ABC",
        // then a back-reference (length=3, disp=3) that re-copies them,
        // producing "ABCABC".
        let source = 0x0300_0000u32;
        let dest = 0x0300_1000u32;
        bus.write32(source, 0x10 | (6 << 8));
        bus.write8(source + 4, 0b0001_0000); // bits: literal,literal,literal,reference,...
        bus.write8(source + 5, b'A');
        bus.write8(source + 6, b'B');
        bus.write8(source + 7, b'C');
        // Reference: length=3 (encoded as 0), disp=3 (encoded as 2).
        bus.write8(source + 8, 0x00); // (length-3)<<4 | high nibble of (disp-1) = 0<<4|0
        bus.write8(source + 9, 0x02); // low byte of (disp-1) = 2 -> disp=3

        lz77_decompress(&mut bus, source, dest);

        let mut out = [0u8; 6];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = bus.read8(dest + i as u32);
        }
        assert_eq!(&out, b"ABCABC");
    }
}
