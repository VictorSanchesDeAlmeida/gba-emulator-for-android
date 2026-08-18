use crate::apu::Apu;
use crate::cartridge::Cartridge;
use crate::dma::{self, Dma};
use crate::interrupts::{InterruptSource, Interrupts};
use crate::joypad::Joypad;
use crate::ppu::Ppu;
use crate::timer::Timers;

const BIOS_SIZE: usize = 0x4000; // 16KB
const EWRAM_SIZE: usize = 0x40000; // 256KB
const IWRAM_SIZE: usize = 0x8000; // 32KB
const IO_SIZE: usize = 0x400; // 1KB total I/O page; see note on `io` field
const PALETTE_SIZE: usize = 0x400; // 1KB
const VRAM_SIZE: usize = 0x18000; // 96KB
const OAM_SIZE: usize = 0x400; // 1KB

const VRAM_MIRROR_PERIOD: usize = 0x20000; // 128KB
const ROM_WINDOW_MASK: usize = 0x01FF_FFFF; // 32MB, one wait-state area

/// The GBA's 32-bit address bus: routes CPU accesses to BIOS, EWRAM,
/// IWRAM, I/O, Palette RAM, VRAM, OAM or the cartridge, and reproduces the
/// mirroring/alignment quirks real games rely on. The CPU only ever sees
/// this `Bus`, never the individual memories directly.
///
/// VRAM, OAM, Palette RAM and the LCD registers (DISPCNT/DISPSTAT/VCOUNT/
/// BGxCNT/...) are physically owned by the [`Ppu`] (real hardware puts them
/// on the PPU chip) — `Bus` still does all the address decoding and
/// mirroring math, it just stores through to `self.ppu` for those regions
/// instead of its own arrays.
///
/// **Open bus** (reading unmapped memory, or BIOS while the CPU isn't
/// executing from it) is the one thing deliberately *not* accurate here:
/// it returns a fixed 0. The correct value depends on the last value that
/// was on the bus (e.g. the CPU's current PC/prefetch).
#[derive(Debug)]
pub struct Bus {
    bios: Vec<u8>,
    ewram: Vec<u8>,
    iwram: Vec<u8>,
    io: Vec<u8>,
    ppu: Ppu,
    joypad: Joypad,
    timers: Timers,
    interrupts: Interrupts,
    apu: Apu,
    dma: Dma,
    cartridge: Cartridge,
}

impl Bus {
    pub fn new(cartridge: Cartridge) -> Self {
        Bus {
            bios: vec![0; BIOS_SIZE],
            ewram: vec![0; EWRAM_SIZE],
            iwram: vec![0; IWRAM_SIZE],
            io: vec![0; IO_SIZE],
            ppu: Ppu::new(),
            joypad: Joypad::new(),
            timers: Timers::new(),
            interrupts: Interrupts::new(),
            apu: Apu::new(),
            dma: Dma::new(),
            cartridge,
        }
    }

    /// Installs a BIOS dump (must be user-provided — Nintendo firmware is
    /// not distributed with this project). Until called, BIOS reads are 0.
    pub fn load_bios(&mut self, data: &[u8]) {
        let n = data.len().min(self.bios.len());
        self.bios[..n].copy_from_slice(&data[..n]);
    }

    /// Zeroes all of EWRAM — used by [`crate::bios`]'s `RegisterRamReset`
    /// HLE.
    pub(crate) fn clear_ewram(&mut self) {
        self.ewram.fill(0);
    }

    /// Zeroes IWRAM except its last 0x200 bytes, matching real hardware's
    /// `RegisterRamReset`: that tail holds the BIOS's own interrupt-check
    /// mirror and reset-parameter area, which a reset must not disturb.
    pub(crate) fn clear_iwram_except_top(&mut self) {
        let preserved_from = self.iwram.len() - 0x200;
        self.iwram[..preserved_from].fill(0);
    }

    pub fn cartridge(&self) -> &Cartridge {
        &self.cartridge
    }

    pub fn cartridge_mut(&mut self) -> &mut Cartridge {
        &mut self.cartridge
    }

    pub fn ppu(&self) -> &Ppu {
        &self.ppu
    }

    pub fn ppu_mut(&mut self) -> &mut Ppu {
        &mut self.ppu
    }

    pub fn joypad(&self) -> &Joypad {
        &self.joypad
    }

    pub fn joypad_mut(&mut self) -> &mut Joypad {
        &mut self.joypad
    }

    pub fn timers(&self) -> &Timers {
        &self.timers
    }

    pub fn timers_mut(&mut self) -> &mut Timers {
        &mut self.timers
    }

    pub fn interrupts(&self) -> &Interrupts {
        &self.interrupts
    }

    pub fn interrupts_mut(&mut self) -> &mut Interrupts {
        &mut self.interrupts
    }

    pub fn apu(&self) -> &Apu {
        &self.apu
    }

    pub fn apu_mut(&mut self) -> &mut Apu {
        &mut self.apu
    }

    pub fn dma(&self) -> &Dma {
        &self.dma
    }

    /// Executes every DMA channel currently armed for `timing` (1=VBlank,
    /// 2=HBlank) — called once per matching PPU event.
    pub fn trigger_dma_for_timing(&mut self, timing: u8) {
        for (channel, &armed) in self.dma.channels_armed_for(timing).iter().enumerate() {
            if armed {
                self.execute_dma_transfer(channel);
            }
        }
    }

    /// Performs one DMA channel's transfer in full, atomically (real
    /// hardware spreads this over cycles and stalls the CPU meanwhile;
    /// this emulator does it as a single step — see the `dma` module doc
    /// for why that's a reasonable simplification here).
    pub(crate) fn execute_dma_transfer(&mut self, channel: usize) {
        let transfer = self.dma.latch_for_transfer(channel);
        let unit = if transfer.word32 { 4 } else { 2 };

        let mut src = transfer.src;
        let mut dst = transfer.dst;
        for _ in 0..transfer.count {
            if transfer.word32 {
                let value = self.read32(src);
                self.write32(dst, value);
            } else {
                let value = self.read16(src);
                self.write16(dst, value);
            }
            src = dma::step_addr(src, transfer.src_control, unit);
            dst = dma::step_addr(dst, transfer.dest_control, unit);
        }

        self.dma.finish_transfer(channel, src, dst);
        if transfer.irq_enable {
            self.interrupts_mut().request(InterruptSource::from_dma_index(channel));
        }
    }

    pub fn read8(&self, addr: u32) -> u8 {
        self.read_region_byte(addr)
    }

    pub fn read16(&self, addr: u32) -> u16 {
        let addr = addr & !1;
        let lo = self.read_region_byte(addr) as u16;
        let hi = self.read_region_byte(addr + 1) as u16;
        lo | (hi << 8)
    }

    pub fn read32(&self, addr: u32) -> u32 {
        let addr = addr & !3;
        let lo = self.read16(addr) as u32;
        let hi = self.read16(addr + 2) as u32;
        lo | (hi << 16)
    }

    /// Byte-granularity store. VRAM and Palette RAM are 16-bit-only
    /// memories: a real GBA duplicates the byte into both halves of the
    /// addressed halfword instead of writing a lone byte. OAM cannot be
    /// byte-written at all and silently ignores the access. See GBATEK
    /// "Video/Palette/OAM Byte Writes".
    pub fn write8(&mut self, addr: u32, value: u8) {
        match region_of(addr) {
            Region::Palette | Region::Vram => {
                let aligned = addr & !1;
                self.write_region_byte(aligned, value);
                self.write_region_byte(aligned + 1, value);
            }
            Region::Oam => {}
            _ => self.write_region_byte(addr, value),
        }
    }

    pub fn write16(&mut self, addr: u32, value: u16) {
        let addr = addr & !1;
        self.write_region_byte(addr, (value & 0xFF) as u8);
        self.write_region_byte(addr + 1, (value >> 8) as u8);
    }

    pub fn write32(&mut self, addr: u32, value: u32) {
        let addr = addr & !3;
        self.write16(addr, (value & 0xFFFF) as u16);
        self.write16(addr + 2, (value >> 16) as u16);
    }

    fn read_region_byte(&self, addr: u32) -> u8 {
        match region_of(addr) {
            Region::Bios => {
                let off = addr as usize;
                if off < self.bios.len() {
                    self.bios[off]
                } else {
                    0
                }
            }
            Region::Ewram => self.ewram[(addr as usize) % EWRAM_SIZE],
            Region::Iwram => self.iwram[(addr as usize) % IWRAM_SIZE],
            Region::Io => {
                let off = (addr as usize) & 0x00FF_FFFF;
                if off < crate::ppu::IO_SIZE {
                    self.ppu.read_register(off)
                } else if let Some(joypad_off) = joypad_offset(off) {
                    self.joypad.read_register(joypad_off)
                } else if let Some(apu_off) = apu_offset(off) {
                    self.apu.read_register(apu_off)
                } else if let Some(dma_off) = dma_offset(off) {
                    self.dma.read_register(dma_off)
                } else if let Some(timer_off) = timer_offset(off) {
                    self.timers.read_register(timer_off)
                } else if let Some(ie_if_off) = ie_if_offset(off) {
                    self.interrupts.read_ie_if(ie_if_off)
                } else if let Some(ime_off) = ime_offset(off) {
                    self.interrupts.read_ime(ime_off)
                } else if off < IO_SIZE {
                    self.io[off]
                } else {
                    0
                }
            }
            Region::Palette => self.ppu.palette()[(addr as usize) % PALETTE_SIZE],
            Region::Vram => self.ppu.vram()[vram_offset(addr)],
            Region::Oam => self.ppu.oam()[(addr as usize) % OAM_SIZE],
            Region::Rom => self.cartridge.read_rom_byte((addr as usize) & ROM_WINDOW_MASK),
            Region::Backup => self.cartridge.read_save_byte((addr as usize) & 0xFFFF),
            Region::Unused => 0,
        }
    }

    /// Plain byte store, region-dispatched but without [`Self::write8`]'s
    /// duplicate-on-write-to-VRAM/Palette or ignore-writes-to-OAM quirks.
    /// Used directly by `write16`/`write32`, which already touch both bytes
    /// of a halfword themselves.
    fn write_region_byte(&mut self, addr: u32, value: u8) {
        match region_of(addr) {
            Region::Bios => {} // read-only
            Region::Ewram => self.ewram[(addr as usize) % EWRAM_SIZE] = value,
            Region::Iwram => self.iwram[(addr as usize) % IWRAM_SIZE] = value,
            Region::Io => {
                let off = (addr as usize) & 0x00FF_FFFF;
                if off < crate::ppu::IO_SIZE {
                    self.ppu.write_register(off, value);
                } else if let Some(joypad_off) = joypad_offset(off) {
                    self.joypad.write_register(joypad_off, value);
                } else if let Some(apu_off) = apu_offset(off) {
                    self.apu.write_register(apu_off, value);
                } else if let Some(dma_off) = dma_offset(off) {
                    self.dma.write_register(dma_off, value);
                    if let Some(channel) = Dma::touched_control_high_byte(dma_off) {
                        if self.dma.armed_for_immediate(channel) {
                            self.execute_dma_transfer(channel);
                        }
                    }
                } else if let Some(timer_off) = timer_offset(off) {
                    self.timers.write_register(timer_off, value);
                } else if let Some(ie_if_off) = ie_if_offset(off) {
                    self.interrupts.write_ie_if(ie_if_off, value);
                } else if let Some(ime_off) = ime_offset(off) {
                    self.interrupts.write_ime(ime_off, value);
                } else if off < IO_SIZE {
                    self.io[off] = value;
                }
            }
            Region::Palette => self.ppu.palette_mut()[(addr as usize) % PALETTE_SIZE] = value,
            Region::Vram => self.ppu.vram_mut()[vram_offset(addr)] = value,
            Region::Oam => self.ppu.oam_mut()[(addr as usize) % OAM_SIZE] = value,
            Region::Rom => {} // cartridge ROM is read-only from the bus
            Region::Backup => self
                .cartridge
                .write_save_byte((addr as usize) & 0xFFFF, value),
            Region::Unused => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Region {
    Bios,
    Ewram,
    Iwram,
    Io,
    Palette,
    Vram,
    Oam,
    Rom,
    Backup,
    Unused,
}

fn region_of(addr: u32) -> Region {
    match ((addr >> 24) & 0xFF) as u8 {
        0x00 => Region::Bios,
        0x02 => Region::Ewram,
        0x03 => Region::Iwram,
        0x04 => Region::Io,
        0x05 => Region::Palette,
        0x06 => Region::Vram,
        0x07 => Region::Oam,
        0x08..=0x0D => Region::Rom,
        0x0E | 0x0F => Region::Backup,
        _ => Region::Unused,
    }
}

/// VRAM is 96KB, not a power of two, so its mirroring is irregular: every
/// 128KB window repeats, but within a window the last 32KB (0x18000..
/// 0x20000) mirrors the *previous* 32KB (0x10000..0x18000) rather than the
/// whole 96KB — see GBATEK "VRAM Mirroring".
fn vram_offset(addr: u32) -> usize {
    let masked = (addr as usize) % VRAM_MIRROR_PERIOD;
    if masked >= VRAM_SIZE {
        masked - 0x8000
    } else {
        masked
    }
}

/// Maps an I/O-page offset to a joypad-relative offset, if it falls within
/// KEYINPUT/KEYCNT (0x130..0x134).
fn joypad_offset(io_offset: usize) -> Option<usize> {
    io_offset.checked_sub(crate::joypad::IO_BASE).filter(|&o| o < crate::joypad::IO_SIZE)
}

/// Maps an I/O-page offset to an APU-relative offset, if it falls within
/// SOUND1CNT_L..FIFO_B (0x60..0xA8).
fn apu_offset(io_offset: usize) -> Option<usize> {
    io_offset.checked_sub(crate::apu::IO_BASE).filter(|&o| o < crate::apu::IO_SIZE)
}

/// Maps an I/O-page offset to a DMA-relative offset, if it falls within
/// DMA0SAD..DMA3CNT_H (0xB0..0xE0).
fn dma_offset(io_offset: usize) -> Option<usize> {
    io_offset.checked_sub(crate::dma::IO_BASE).filter(|&o| o < crate::dma::IO_SIZE)
}

/// Maps an I/O-page offset to a timer-relative offset, if it falls within
/// TM0CNT_L..TM3CNT_H (0x100..0x110).
fn timer_offset(io_offset: usize) -> Option<usize> {
    io_offset.checked_sub(crate::timer::IO_BASE).filter(|&o| o < crate::timer::IO_SIZE)
}

/// Maps an I/O-page offset to an IE/IF-relative offset (0x200..0x204).
fn ie_if_offset(io_offset: usize) -> Option<usize> {
    io_offset.checked_sub(crate::interrupts::IE_IF_BASE).filter(|&o| o < crate::interrupts::IE_IF_SIZE)
}

/// Maps an I/O-page offset to an IME-relative offset (0x208..0x20C).
fn ime_offset(io_offset: usize) -> Option<usize> {
    io_offset.checked_sub(crate::interrupts::IME_BASE).filter(|&o| o < crate::interrupts::IME_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cartridge::HEADER_SIZE;

    fn test_bus() -> Bus {
        let rom = vec![0u8; HEADER_SIZE + 16];
        Bus::new(Cartridge::load(rom).unwrap())
    }

    #[test]
    fn ewram_read_write_and_mirroring() {
        let mut bus = test_bus();
        bus.write8(0x0200_1234, 0xAB);
        assert_eq!(bus.read8(0x0200_1234), 0xAB);
        assert_eq!(
            bus.read8(0x0200_1234 + EWRAM_SIZE as u32),
            0xAB,
            "EWRAM mirrors every 256KB"
        );
    }

    #[test]
    fn iwram_read_write_and_mirroring() {
        let mut bus = test_bus();
        bus.write8(0x0300_0010, 0x7A);
        assert_eq!(bus.read8(0x0300_0010), 0x7A);
        assert_eq!(
            bus.read8(0x0300_0010 + IWRAM_SIZE as u32),
            0x7A,
            "IWRAM mirrors every 32KB"
        );
    }

    #[test]
    fn bios_is_zero_until_loaded_and_ignores_writes() {
        let mut bus = test_bus();
        assert_eq!(bus.read8(0x0000_0010), 0);
        bus.write8(0x0000_0010, 0xFF);
        assert_eq!(bus.read8(0x0000_0010), 0, "BIOS is read-only");

        bus.load_bios(&[0x11, 0x22, 0x33]);
        assert_eq!(bus.read8(0x0000_0000), 0x11);
        assert_eq!(bus.read8(0x0000_0002), 0x33);
    }

    #[test]
    fn io_read_write_roundtrip_and_out_of_range_is_open_bus() {
        let mut bus = test_bus();
        // 0xB0 = a DMA transfer register (DMA0SAD): no general DMA
        // controller exists yet, so it's still a plain placeholder byte.
        bus.write8(0x0400_00B0, 0x5A);
        assert_eq!(bus.read8(0x0400_00B0), 0x5A);
        assert_eq!(bus.read8(0x0400_0FFF), 0, "past the placeholder I/O block");
    }

    #[test]
    fn ppu_lcd_registers_route_through_the_bus_to_the_ppu() {
        let mut bus = test_bus();
        bus.write16(0x0400_0000, 0x0403); // DISPCNT: mode 3, BG2 not yet enabled here
        assert_eq!(bus.ppu().dispcnt(), 0x0403);
        assert_eq!(bus.ppu().bg_mode(), 3);
    }

    #[test]
    fn keyinput_is_readable_through_the_bus_and_reflects_pressed_buttons() {
        let mut bus = test_bus();
        assert_eq!(bus.read16(0x0400_0130), 0xFFFF, "every button starts released");

        bus.joypad_mut().press(crate::joypad::Button::A);
        assert_eq!(bus.read16(0x0400_0130) & 0x01, 0, "A's bit goes low when held");

        bus.write16(0x0400_0130, 0x0000); // KEYINPUT is read-only
        assert_eq!(bus.read16(0x0400_0130) & 0x01, 0, "write must not un-press A");
    }

    #[test]
    fn timer_registers_route_through_the_bus() {
        let mut bus = test_bus();
        bus.write16(0x0400_0100, 0xFFF0); // TM0CNT_L reload
        bus.write16(0x0400_0102, 0x0080); // TM0CNT_H: enable, prescaler=1
        assert_eq!(bus.read16(0x0400_0100), 0xFFF0, "enabling loads the counter immediately");

        bus.timers_mut().tick(4);
        assert_eq!(bus.read16(0x0400_0100), 0xFFF4);
    }

    #[test]
    fn interrupt_registers_route_through_the_bus() {
        let mut bus = test_bus();
        bus.write16(0x0400_0200, 0x0001); // IE: VBlank
        bus.write16(0x0400_0208, 0x0001); // IME on
        assert_eq!(bus.read16(0x0400_0200), 0x0001);
        assert_eq!(bus.read16(0x0400_0208), 0x0001);

        assert!(!bus.interrupts().pending());
        bus.interrupts_mut().request(crate::interrupts::InterruptSource::VBlank);
        assert!(bus.interrupts().pending());
        assert_eq!(bus.read16(0x0400_0202) & 0x01, 0x01, "IF must reflect the pending request");

        bus.write16(0x0400_0202, 0x0001); // acknowledge
        assert!(!bus.interrupts().pending());
    }

    #[test]
    fn apu_registers_route_through_the_bus() {
        let mut bus = test_bus();
        bus.write8(0x0400_0090, 0xAB); // wave RAM byte 0
        assert_eq!(bus.read8(0x0400_0090), 0xAB);

        bus.write8(0x0400_0084, 0x80); // SOUNDCNT_X: master enable
        assert_eq!(bus.read8(0x0400_0084) & 0x80, 0x80);
    }

    #[test]
    fn immediate_dma_fires_the_instant_control_h_arms_it() {
        let mut bus = test_bus();
        bus.write32(0x0300_0000, 0x1122_3344); // source word in IWRAM
        bus.write32(0x0400_00B0, 0x0300_0000); // DMA0SAD
        bus.write32(0x0400_00B4, 0x0300_1000); // DMA0DAD
        bus.write16(0x0400_00B8, 1); // DMA0CNT_L: 1 unit
        bus.write16(0x0400_00BA, 0x8400); // DMA0CNT_H: enable, 32-bit, immediate

        assert_eq!(bus.read32(0x0300_1000), 0x1122_3344, "transfer must run synchronously on arm");
        assert_eq!(bus.read16(0x0400_00BA) & 0x8000, 0, "non-repeat channel must auto-disable after firing");
    }

    #[test]
    fn dma_with_fixed_source_control_repeats_the_same_word() {
        let mut bus = test_bus();
        bus.write32(0x0300_0000, 0xCAFE_BABE);
        bus.write32(0x0400_00B0, 0x0300_0000); // SAD
        bus.write32(0x0400_00B4, 0x0300_2000); // DAD
        bus.write16(0x0400_00B8, 3); // 3 units
        // control: enable, 32-bit, immediate, src=Fixed(0b10 at bits 7-8), dest=Increment(0)
        bus.write16(0x0400_00BA, 0x8400 | (0b10 << 7));

        assert_eq!(bus.read32(0x0300_2000), 0xCAFE_BABE);
        assert_eq!(bus.read32(0x0300_2004), 0xCAFE_BABE);
        assert_eq!(bus.read32(0x0300_2008), 0xCAFE_BABE);
    }

    #[test]
    fn dma_irq_enable_requests_the_matching_interrupt_source() {
        let mut bus = test_bus();
        bus.write16(0x0400_0200, 0x0100); // IE: DMA0 (bit8)
        bus.write16(0x0400_0208, 0x0001); // IME on
        bus.write32(0x0400_00B0, 0x0300_0000);
        bus.write32(0x0400_00B4, 0x0300_1000);
        bus.write16(0x0400_00B8, 1);
        bus.write16(0x0400_00BA, 0x8400 | 0x4000); // enable, 32-bit, immediate, IRQ enable

        assert!(bus.interrupts().pending(), "DMA0's completion IRQ must be requested");
    }

    #[test]
    fn vblank_triggers_an_armed_dma_channel() {
        let mut bus = test_bus();
        bus.write32(0x0300_0000, 0x9999_9999);
        bus.write32(0x0400_00B0, 0x0300_0000);
        bus.write32(0x0400_00B4, 0x0300_3000);
        bus.write16(0x0400_00B8, 1);
        bus.write16(0x0400_00BA, 0x9400); // enable, 32-bit, VBlank timing
        assert_eq!(bus.read32(0x0300_3000), 0, "must not fire until VBlank actually occurs");

        bus.trigger_dma_for_timing(1);
        assert_eq!(bus.read32(0x0300_3000), 0x9999_9999);
    }

    #[test]
    fn keycnt_is_writable_through_the_bus() {
        let mut bus = test_bus();
        bus.write16(0x0400_0132, 0xC003); // select A+B, IRQ enable, AND mode
        assert_eq!(bus.read16(0x0400_0132), 0xC003);
    }

    #[test]
    fn palette_byte_write_duplicates_across_the_halfword() {
        let mut bus = test_bus();
        bus.write8(0x0500_0000, 0x3C);
        assert_eq!(bus.read16(0x0500_0000), 0x3C3C);
    }

    #[test]
    fn oam_byte_writes_are_ignored() {
        let mut bus = test_bus();
        bus.write16(0x0700_0000, 0x1234);
        bus.write8(0x0700_0000, 0xFF);
        assert_eq!(
            bus.read16(0x0700_0000),
            0x1234,
            "OAM cannot be byte-written on real hardware"
        );
    }

    #[test]
    fn vram_byte_write_duplicates_and_mirrors_past_96kb() {
        let mut bus = test_bus();
        bus.write8(0x0601_0005, 0x77);
        assert_eq!(bus.read16(0x0601_0004), 0x7777);

        // 0x06018000..0x06020000 mirrors 0x06010000..0x06018000, not the
        // whole 96KB region.
        assert_eq!(bus.read8(0x0601_8005), 0x77);
    }

    #[test]
    fn rom_is_mirrored_across_all_three_wait_state_windows() {
        let mut rom = vec![0u8; HEADER_SIZE + 16];
        rom[HEADER_SIZE] = 0x42;
        let bus = Bus::new(Cartridge::load(rom).unwrap());

        let offset = HEADER_SIZE as u32;
        assert_eq!(bus.read8(0x0800_0000 + offset), 0x42);
        assert_eq!(bus.read8(0x0A00_0000 + offset), 0x42);
        assert_eq!(bus.read8(0x0C00_0000 + offset), 0x42);
    }

    #[test]
    fn backup_memory_writes_route_to_the_cartridge_save_backend() {
        let mut rom = vec![0u8; HEADER_SIZE + 16];
        rom[HEADER_SIZE..HEADER_SIZE + 6].copy_from_slice(b"SRAM_V");
        let mut bus = Bus::new(Cartridge::load(rom).unwrap());

        bus.write8(0x0E00_0010, 0x99);
        assert_eq!(bus.read8(0x0E00_0010), 0x99);
        assert_eq!(bus.cartridge().read_save_byte(0x10), 0x99);
    }

    #[test]
    fn word_and_halfword_access_is_little_endian() {
        let mut bus = test_bus();
        bus.write32(0x0300_0000, 0x1122_3344);
        assert_eq!(bus.read8(0x0300_0000), 0x44);
        assert_eq!(bus.read8(0x0300_0001), 0x33);
        assert_eq!(bus.read8(0x0300_0002), 0x22);
        assert_eq!(bus.read8(0x0300_0003), 0x11);
        assert_eq!(bus.read16(0x0300_0002), 0x1122);
        assert_eq!(bus.read32(0x0300_0000), 0x1122_3344);
    }
}
