use crate::bios;
use crate::cartridge::Cartridge;
use crate::cpu::{psr, Cpu, CpuError, Mode};
use crate::interrupts::InterruptSource;
use crate::memory::Bus;

/// Where the cartridge's own entry-point branch lives — every commercial
/// GBA ROM starts here.
const CARTRIDGE_ENTRY: u32 = 0x0800_0000;

/// The GBA's fixed CPU clock: 2^24 Hz (~16.78 MHz).
pub const CPU_CLOCK_HZ: u32 = 16_777_216;
/// Cycles per scanline (240 visible dots + 68 H-blank dots, 4 cycles/dot).
pub const CYCLES_PER_SCANLINE: u32 = 1232;
/// Scanlines per frame (160 visible + 68 V-blank).
pub const SCANLINES_PER_FRAME: u32 = 228;
/// Cycles per video frame at the GBA's native ~59.7275 Hz refresh rate.
pub const CYCLES_PER_FRAME: u32 = CYCLES_PER_SCANLINE * SCANLINES_PER_FRAME;

/// Ties the CPU to the Bus and turns per-instruction cycle counts into a
/// schedulable clock, instead of a wall-clock loop (`setInterval`-style
/// timing is explicitly what the GBA's CPU/PPU/Timer/APU synchronization
/// must NOT be built on — they all derive from this same cycle count).
///
/// PPU (Etapa 6), Timers, interrupt dispatch (Etapa 9) and the APU
/// (Etapa 10) all tick here. [`Self::step`] also intercepts `SWI` calls
/// and the (synthetic) IRQ vector for [`bios`]'s HLE — see that module's
/// doc comment for why.
pub struct Emulator {
    pub cpu: Cpu,
    pub bus: Bus,
    cycles: u64,
    /// `Some(mask)` while halted by `Halt`/`IntrWait`/`VBlankIntrWait`:
    /// game code doesn't run again until the RAM mirror those functions
    /// poll matches `mask` — see [`bios::check_halt_wakeup`].
    halt_mask: Option<u16>,
    /// Whether [`Self::step`] should intercept `SWI`/the IRQ vector with
    /// [`bios`]'s HLE. `true` unless constructed via
    /// [`Self::new_with_bios`]: with a real BIOS dump installed, that ROM's
    /// own code actually lives at the SWI/IRQ vectors, so interception
    /// would just steal control from it instead of letting it run.
    hle_bios: bool,
}

impl Emulator {
    /// Constructs an `Emulator` and boots it straight into the cartridge,
    /// replicating the handful of things a real GBA BIOS's reset handler
    /// does before handing off to game code — this project doesn't ship
    /// (and can't legally ship) a BIOS dump, so this stands in for it.
    /// Without this, [`Cpu::new`]'s hardware-reset state (PC=0, Supervisor
    /// mode, every stack pointer still 0) would leave the CPU executing
    /// whatever's at address 0 forever (all zeros — silently doing
    /// nothing) instead of ever reaching the cartridge, and the first
    /// `PUSH`/stack access any real game makes would corrupt low memory
    /// instead of using a real stack.
    pub fn new(cartridge: Cartridge) -> Self {
        let mut emu =
            Emulator { cpu: Cpu::new(), bus: Bus::new(cartridge), cycles: 0, halt_mask: None, hle_bios: true };
        emu.boot_without_bios();
        emu
    }

    /// Constructs an `Emulator` that boots a real, user-supplied BIOS dump
    /// (installed via [`Bus::load_bios`]) instead of [`Self::new`]'s
    /// HLE stand-in. [`Cpu::new`]'s hardware-reset state (PC=0, ARM state,
    /// Supervisor mode, every banked SP still 0) is already exactly what
    /// real GBA hardware presents to a BIOS on power-up — the BIOS's own
    /// reset handler is responsible for setting up stack pointers and
    /// jumping to the cartridge, so unlike [`Self::new`] nothing here needs
    /// to fake that. [`Self::step`] also stops intercepting `SWI`/IRQ: the
    /// real dumped code at those vectors runs for real instead.
    pub fn new_with_bios(cartridge: Cartridge, bios: &[u8]) -> Self {
        let mut bus = Bus::new(cartridge);
        bus.load_bios(bios);
        Emulator { cpu: Cpu::new(), bus, cycles: 0, halt_mask: None, hle_bios: false }
    }

    /// Stack pointer values and starting mode/state a real BIOS sets up
    /// before jumping to the cartridge (see GBATEK "BIOS RAM Usage" and
    /// the well-documented open-source BIOS-replacement boot sequences).
    /// Also `SoftReset`'s (SWI 0x00) HLE — see [`bios::try_handle_swi`].
    fn boot_without_bios(&mut self) {
        self.halt_mask = None;
        self.cpu.registers.set_mode(Mode::Irq);
        self.cpu.registers.set_r(13, 0x0300_7FA0);
        self.cpu.registers.set_mode(Mode::Supervisor);
        self.cpu.registers.set_r(13, 0x0300_7FE0);
        self.cpu.registers.set_mode(Mode::System);
        self.cpu.registers.set_r(13, 0x0300_7F00);
        self.cpu.registers.set_flag(psr::I, false); // CPU-level IRQ gate open; games rely on IME to actually mask
        self.cpu.registers.set_flag(psr::T, false); // cartridge entry is always an ARM-state B instruction
        self.cpu.registers.set_pc(CARTRIDGE_ENTRY);
    }

    /// Total cycles executed since this `Emulator` was created.
    pub fn total_cycles(&self) -> u64 {
        self.cycles
    }

    /// Executes exactly one CPU instruction and accounts for its cycles —
    /// or, in priority order:
    /// 1. If an interrupt is pending and the CPU has IRQs enabled, takes
    ///    the IRQ exception (checked at this same instruction boundary,
    ///    which is also where real ARM7TDMI hardware samples it).
    /// 2. If PC just landed on the (synthetic) IRQ vector, services it —
    ///    see [`bios`].
    /// 3. If halted (`Halt`/`IntrWait`/`VBlankIntrWait`), ticks hardware
    ///    without running game code until the awaited condition occurs.
    /// 4. If the next instruction is an `SWI` this project HLEs, performs
    ///    its effect directly instead of executing it.
    /// 5. Otherwise, executes the instruction normally.
    pub fn step(&mut self) -> Result<u32, CpuError> {
        if self.bus.interrupts().pending() && !self.cpu.registers.flag(psr::I) {
            self.cpu.enter_irq();
            self.cycles += 3;
            return Ok(3);
        }

        if self.hle_bios && self.cpu.registers.pc() == bios::IRQ_VECTOR && self.cpu.registers.mode() == Mode::Irq {
            bios::service_interrupt(&mut self.cpu.registers, &mut self.bus);
            self.cycles += 3;
            return Ok(3);
        }

        if self.hle_bios
            && self.cpu.registers.pc() == bios::IRQ_RETURN_STUB
            && self.cpu.registers.mode() == Mode::Irq
        {
            bios::finish_interrupt_after_handler(&mut self.cpu.registers, &mut self.bus);
            self.cycles += 3;
            return Ok(3);
        }

        if let Some(mask) = self.halt_mask {
            // Checked every step while halted, independent of the real
            // interrupt-dispatch branch above: `Halt`/`IntrWait`/
            // `VBlankIntrWait` wake on raw IE&IF, not on IME — see
            // `bios::check_halt_wakeup`.
            if bios::check_halt_wakeup(&mut self.bus, mask) {
                self.halt_mask = None;
            } else {
                let cycles = 8;
                self.cycles += cycles as u64;
                self.tick_peripherals(cycles);
                return Ok(cycles);
            }
        }

        if self.hle_bios {
            if let Some(outcome) = bios::try_handle_swi(&mut self.cpu.registers, &mut self.bus) {
                match outcome {
                    bios::SwiOutcome::SoftReset => self.boot_without_bios(),
                    bios::SwiOutcome::Handled { halt_mask } => {
                        if halt_mask.is_some() {
                            self.halt_mask = halt_mask;
                        }
                    }
                }
                self.cycles += 3;
                return Ok(3);
            }
        }

        let cycles = self.cpu.step(&mut self.bus)?;
        self.cycles += cycles as u64;
        self.tick_peripherals(cycles);
        Ok(cycles)
    }

    /// Advances PPU/Timers/APU by `cycles`, turns their newly-fired
    /// conditions into interrupt requests, and fires any DMA channel
    /// armed for VBlank/HBlank — the shared tail of both normal
    /// instruction execution and halted idle-ticking.
    fn tick_peripherals(&mut self, cycles: u32) {
        let ppu_events = self.bus.ppu_mut().tick(cycles);
        let timer_events = self.bus.timers_mut().tick(cycles);

        if ppu_events.vblank {
            self.bus.interrupts_mut().request(InterruptSource::VBlank);
        }
        if ppu_events.hblank {
            self.bus.interrupts_mut().request(InterruptSource::HBlank);
        }
        if ppu_events.vcounter {
            self.bus.interrupts_mut().request(InterruptSource::VCounter);
        }
        // DMA's VBlank/HBlank start timing is a raw hardware signal, not
        // gated by whether the matching IRQ is enabled — see `IrqEvents`.
        if ppu_events.vblank_timing {
            self.bus.trigger_dma_for_timing(1);
        }
        if ppu_events.hblank_timing {
            self.bus.trigger_dma_for_timing(2);
        }
        let mut timer_overflowed = [false; 4];
        for i in 0..4 {
            timer_overflowed[i] = timer_events.overflowed(i);
            if timer_overflowed[i] {
                self.bus.interrupts_mut().request(InterruptSource::from_timer_index(i));
            }
        }
        self.bus.apu_mut().tick(cycles, timer_overflowed);
    }

    /// Runs whole instructions until at least `target_cycles` have elapsed.
    /// Accounting is at instruction-boundary granularity: since an
    /// instruction can't be subdivided, the last one may slightly overshoot
    /// `target_cycles` — this returns the actual number of cycles run.
    pub fn run_cycles(&mut self, target_cycles: u32) -> Result<u32, CpuError> {
        let mut ran = 0u32;
        while ran < target_cycles {
            ran += self.step()?;
        }
        Ok(ran)
    }

    /// Runs approximately one video frame's worth of cycles.
    pub fn run_frame(&mut self) -> Result<u32, CpuError> {
        self.run_cycles(CYCLES_PER_FRAME)
    }

    /// Sends a discrete button event to the joypad. The UI layer only ever
    /// calls these two methods — it never touches emulation state itself.
    pub fn press_key(&mut self, button: crate::joypad::Button) {
        self.bus.joypad_mut().press(button);
    }

    pub fn release_key(&mut self, button: crate::joypad::Button) {
        self.bus.joypad_mut().release(button);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::HEADER_SIZE;

    fn emulator_with_program(program: &[u32]) -> Emulator {
        let rom = vec![0u8; HEADER_SIZE + 16];
        let mut emu = Emulator::new(Cartridge::load(rom).unwrap());
        let base = 0x0300_0000u32;
        for (i, &word) in program.iter().enumerate() {
            emu.bus.write32(base + (i as u32) * 4, word);
        }
        emu.cpu.registers.set_pc(base);
        emu
    }

    fn encode_dp(cond: u32, opcode4: u32, s: u32, rn: u32, rd: u32, operand2: u32, immediate: bool) -> u32 {
        (cond << 28) | ((immediate as u32) << 25) | (opcode4 << 21) | (s << 20) | (rn << 16) | (rd << 12) | operand2
    }

    #[test]
    fn step_accounts_cycles_and_advances_pc() {
        let mut emu = emulator_with_program(&[
            encode_dp(0xE, 0xD, 0, 0, 0, 1, true), // MOV r0, #1
        ]);
        let cycles = emu.step().unwrap();
        assert!(cycles >= 1);
        assert_eq!(emu.total_cycles(), cycles as u64);
        assert_eq!(emu.cpu.registers.pc(), 0x0300_0004);
    }

    #[test]
    fn run_cycles_runs_at_least_the_requested_budget() {
        // Four MOVs, each costing 1 cycle at this etapa's approximation.
        let mut emu = emulator_with_program(&[
            encode_dp(0xE, 0xD, 0, 0, 0, 1, true),
            encode_dp(0xE, 0xD, 0, 0, 1, 2, true),
            encode_dp(0xE, 0xD, 0, 0, 2, 3, true),
            encode_dp(0xE, 0xD, 0, 0, 3, 4, true),
        ]);
        let ran = emu.run_cycles(3).unwrap();
        assert!(ran >= 3, "must not stop short of the requested budget");
        assert!(emu.cpu.registers.r(2) == 3 || emu.cpu.registers.r(3) == 4, "enough instructions actually ran");
    }

    #[test]
    fn run_cycles_propagates_an_unimplemented_instruction_as_an_error() {
        // A coprocessor data-operation encoding (bits27-24=1110, bit4=0):
        // GBA has no coprocessor, so this is deliberately unimplemented.
        let mut emu = emulator_with_program(&[0xEE00_0000]);
        let err = emu.run_cycles(10).unwrap_err();
        assert!(matches!(err, crate::cpu::CpuError::UnimplementedArm { .. }));
    }

    #[test]
    fn cycle_constants_are_internally_consistent() {
        assert_eq!(CYCLES_PER_FRAME, CYCLES_PER_SCANLINE * SCANLINES_PER_FRAME);
        assert_eq!(CYCLES_PER_FRAME, 280_896);
    }

    /// The whole point of Etapa 9: a real game's boot sequence spins on
    /// "wait for VBlank" (`B $`, branch to self) until the LCD raises its
    /// VBlank interrupt — proving that end-to-end here (PPU condition -> IF
    /// bit -> CPU exception entry) is what actually gets real ROMs past
    /// their init loop and onto drawing something.
    #[test]
    fn vblank_irq_interrupts_a_spin_wait_loop() {
        let rom = vec![0u8; HEADER_SIZE + 16];
        let mut emu = Emulator::new(Cartridge::load(rom).unwrap());

        let base = 0x0300_0000u32;
        emu.bus.write32(base, 0xEAFF_FFFE); // B $
        emu.cpu.registers.set_pc(base);
        emu.cpu.registers.set_mode(crate::cpu::Mode::System);
        emu.cpu.registers.set_flag(crate::cpu::psr::I, false);

        emu.bus.write16(0x0400_0004, 0x0008); // DISPSTAT: VBlank IRQ enable
        emu.bus.write16(0x0400_0200, 0x0001); // IE: VBlank
        emu.bus.write16(0x0400_0208, 0x0001); // IME on

        let mut took_irq = false;
        for _ in 0..(CYCLES_PER_FRAME * 2) {
            emu.step().unwrap();
            if emu.cpu.registers.mode() == crate::cpu::Mode::Irq {
                took_irq = true;
                break;
            }
        }

        assert!(took_irq, "a VBlank IRQ must eventually interrupt the spin loop");
        assert_eq!(emu.cpu.registers.pc(), 0x0000_0018);

        // The next step runs the synthetic IRQ servicing (bios::service_interrupt),
        // which must return all the way back to the spin loop, in System mode.
        emu.step().unwrap();
        assert_eq!(emu.cpu.registers.mode(), crate::cpu::Mode::System, "fully returned from the interrupt");
        assert_eq!(emu.cpu.registers.pc(), base, "back at the spin loop instruction");
    }

    #[test]
    fn timer_overflow_irq_interrupts_a_spin_wait_loop() {
        let rom = vec![0u8; HEADER_SIZE + 16];
        let mut emu = Emulator::new(Cartridge::load(rom).unwrap());

        let base = 0x0300_0000u32;
        emu.bus.write32(base, 0xEAFF_FFFE); // B $
        emu.cpu.registers.set_pc(base);
        emu.cpu.registers.set_mode(crate::cpu::Mode::System);
        emu.cpu.registers.set_flag(crate::cpu::psr::I, false);

        emu.bus.write16(0x0400_0100, 0xFFFE); // TM0CNT_L: overflow in 2 cycles
        emu.bus.write16(0x0400_0102, 0x00C0); // TM0CNT_H: enable + IRQ, prescaler=1
        emu.bus.write16(0x0400_0200, 0x0008); // IE: Timer0 (bit3)
        emu.bus.write16(0x0400_0208, 0x0001); // IME on

        let mut took_irq = false;
        for _ in 0..1000 {
            emu.step().unwrap();
            if emu.cpu.registers.mode() == crate::cpu::Mode::Irq {
                took_irq = true;
                break;
            }
        }

        assert!(took_irq, "a Timer0 overflow IRQ must eventually interrupt the spin loop");
        assert_eq!(emu.cpu.registers.pc(), 0x0000_0018);
    }

    /// The other half of "what actually gets a real game playable": a real
    /// game's main loop calls `VBlankIntrWait` (SWI 0x05) every frame — an
    /// SWI this project HLEs since there's no BIOS to run it for real. This
    /// proves the whole chain end to end: the SWI halts the CPU without
    /// executing whatever garbage sits after it, a real VBlank interrupt
    /// eventually fires, the synthetic BIOS servicing (`bios::
    /// service_interrupt`) acknowledges it and updates the RAM mirror
    /// `VBlankIntrWait` polls, and execution resumes.
    #[test]
    fn vblank_intr_wait_halts_until_vblank_then_resumes() {
        let rom = vec![0u8; HEADER_SIZE + 16];
        let mut emu = Emulator::new(Cartridge::load(rom).unwrap());

        let base = 0x0300_0000u32;
        emu.bus.write32(base, 0xEF05_0000); // SWI 0x05 (VBlankIntrWait)
        emu.cpu.registers.set_pc(base);

        emu.bus.write16(0x0400_0004, 0x0008); // DISPSTAT: VBlank IRQ enable
        emu.bus.write16(0x0400_0200, 0x0001); // IE: VBlank
        emu.bus.write16(0x0400_0208, 0x0001); // IME on

        emu.step().unwrap(); // the SWI itself: HLE'd, halts
        assert_eq!(emu.cpu.registers.pc(), base + 4, "PC must advance past the SWI");

        for _ in 0..20 {
            emu.step().unwrap();
        }
        assert_eq!(emu.cpu.registers.pc(), base + 4, "must stay halted well before a VBlank can occur");

        let mut woke = false;
        for _ in 0..(CYCLES_PER_FRAME / 8 + 1000) {
            emu.step().unwrap();
            if emu.cpu.registers.pc() != base + 4 {
                woke = true;
                break;
            }
        }
        assert!(woke, "must resume once VBlank fires and gets serviced");
    }

    /// A real, previously-live bug: `Halt`/`IntrWait`/`VBlankIntrWait` wake
    /// on raw IE&IF, completely independent of IME — real hardware
    /// behavior, and exactly the state a game is in seconds into boot
    /// (DISPSTAT/IE already configured for VBlank, IME not set yet). The
    /// earlier (wrong) implementation gated the wakeup on IME too, via the
    /// RAM mirror only being updated by IME-gated interrupt servicing —
    /// which deadlocked real ROMs forever the first time they called
    /// `VBlankIntrWait` before their own "enable interrupts" step.
    #[test]
    fn vblank_intr_wait_wakes_up_even_while_ime_is_still_off() {
        let rom = vec![0u8; HEADER_SIZE + 16];
        let mut emu = Emulator::new(Cartridge::load(rom).unwrap());

        let base = 0x0300_0000u32;
        emu.bus.write32(base, 0xEF05_0000); // SWI 0x05 (VBlankIntrWait)
        emu.cpu.registers.set_pc(base);

        emu.bus.write16(0x0400_0004, 0x0008); // DISPSTAT: VBlank IRQ enable
        emu.bus.write16(0x0400_0200, 0x0001); // IE: VBlank
        // IME is deliberately left off (0x0400_0208 never written).

        emu.step().unwrap(); // the SWI itself: HLE'd, halts
        assert_eq!(emu.cpu.registers.pc(), base + 4);

        let mut woke = false;
        for _ in 0..(CYCLES_PER_FRAME / 8 + 1000) {
            emu.step().unwrap();
            if emu.cpu.registers.pc() != base + 4 {
                woke = true;
                break;
            }
        }
        assert!(woke, "Halt-derived waits must not require IME to wake up");
    }

    #[test]
    fn unimplemented_swi_number_is_a_no_op_not_a_hang() {
        let rom = vec![0u8; HEADER_SIZE + 16];
        let mut emu = Emulator::new(Cartridge::load(rom).unwrap());

        let base = 0x0300_0000u32;
        emu.bus.write32(base, 0xEF63_0000); // SWI 0x63: not implemented by this project
        emu.cpu.registers.set_pc(base);

        emu.step().unwrap();
        assert_eq!(emu.cpu.registers.pc(), base + 4, "must still advance past the SWI instead of hanging");
    }

    #[test]
    fn soft_reset_swi_reboots_to_the_cartridge_entry() {
        let rom = vec![0u8; HEADER_SIZE + 16];
        let mut emu = Emulator::new(Cartridge::load(rom).unwrap());

        let base = 0x0300_0000u32;
        emu.bus.write32(base, 0xEF00_0000); // SWI 0x00 (SoftReset)
        emu.cpu.registers.set_pc(base);
        emu.cpu.registers.set_r(4, 0x1234); // arbitrary pre-reset state

        emu.step().unwrap();
        assert_eq!(emu.cpu.registers.pc(), 0x0800_0000, "SoftReset must reboot to the cartridge entry");
        assert_eq!(emu.cpu.registers.mode(), crate::cpu::Mode::System);
    }
}
