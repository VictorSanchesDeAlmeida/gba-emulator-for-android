//! The GBA's 4 general-purpose DMA channels (DMA0-3): background,
//! event-triggered, memory-to-memory block transfers. Real games rely on
//! these constantly — copying decompressed graphics into VRAM, palette
//! data, tilemaps — and a lot of real game code just arms a channel and
//! waits for hardware to do the copy, rather than calling a BIOS function
//! like `CpuSet`.
//!
//! This module only holds each channel's register *state* — reading it
//! and deciding when a channel is armed. It cannot perform a transfer
//! itself (that needs read/write access to the *entire* address space,
//! not just this module's own registers, the same reason [`crate::bios`]
//! takes a `Bus` parameter instead of being a bus-owned component with
//! purely-local state). [`crate::memory::Bus::execute_dma_transfer`] does
//! the actual copy, using [`Dma::latch_for_transfer`] to read out what
//! this module has recorded.
//!
//! Scope: Immediate, VBlank and HBlank start timing are fully
//! implemented — a transfer executes atomically (all at once, not spread
//! over real cycles) the instant its trigger condition occurs. "Special"
//! timing (DMA1/2 Direct Sound FIFO auto-refill, DMA3 video capture) is
//! not implemented: a channel armed with Special timing simply never
//! fires. Direct Sound can still be fed by a game writing FIFO_A/B
//! directly (already wired in [`crate::apu`]) — only the DMA-driven
//! auto-refill path some games use for smoother mixing is affected.

pub const IO_BASE: usize = 0xB0;
pub(crate) const IO_SIZE: usize = 0x30; // 4 channels * 12 bytes

/// DMAxCNT_H's 2-bit address-control encoding (source: bits 7-8, dest:
/// bits 5-6 — dest is the only one where `IncrementReload` is meaningful).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AddrControl {
    Increment,
    Decrement,
    Fixed,
    IncrementReload,
}

impl AddrControl {
    fn from_bits(bits: u16) -> Self {
        match bits & 0x3 {
            0 => AddrControl::Increment,
            1 => AddrControl::Decrement,
            2 => AddrControl::Fixed,
            _ => AddrControl::IncrementReload,
        }
    }
}

#[derive(Debug)]
struct DmaChannel {
    sad: u32,
    dad: u32,
    count: u16,
    control: u16,
}

impl DmaChannel {
    fn new() -> Self {
        DmaChannel { sad: 0, dad: 0, count: 0, control: 0 }
    }

    fn enabled(&self) -> bool {
        self.control & 0x8000 != 0
    }
    fn irq_enable(&self) -> bool {
        self.control & 0x4000 != 0
    }
    /// 0=Immediate, 1=VBlank, 2=HBlank, 3=Special.
    fn start_timing(&self) -> u8 {
        ((self.control >> 12) & 0x3) as u8
    }
    fn word32(&self) -> bool {
        self.control & 0x0400 != 0
    }
    fn repeat(&self) -> bool {
        self.control & 0x0200 != 0
    }
    fn src_control(&self) -> AddrControl {
        AddrControl::from_bits(self.control >> 7)
    }
    fn dest_control(&self) -> AddrControl {
        AddrControl::from_bits(self.control >> 5)
    }

    /// The raw count register, expanded per real hardware's "0 means
    /// max" quirk: DMA0-2 have a 14-bit count field (max 0x4000), DMA3
    /// has a full 16-bit field (max 0x10000).
    fn effective_count(&self, channel_index: usize) -> u32 {
        let mask = if channel_index == 3 { 0xFFFF } else { 0x3FFF };
        let max = if channel_index == 3 { 0x1_0000 } else { 0x4000 };
        let masked = self.count as u32 & mask;
        if masked == 0 {
            max
        } else {
            masked
        }
    }

    fn disable(&mut self) {
        self.control &= !0x8000;
    }
}

#[derive(Debug)]
pub struct Dma {
    channels: [DmaChannel; 4],
}

impl Dma {
    pub fn new() -> Self {
        Dma { channels: [DmaChannel::new(), DmaChannel::new(), DmaChannel::new(), DmaChannel::new()] }
    }

    pub(crate) fn read_register(&self, offset: usize) -> u8 {
        let ch = &self.channels[offset / 12];
        match offset % 12 {
            0 => (ch.sad & 0xFF) as u8,
            1 => (ch.sad >> 8) as u8,
            2 => (ch.sad >> 16) as u8,
            3 => (ch.sad >> 24) as u8,
            4 => (ch.dad & 0xFF) as u8,
            5 => (ch.dad >> 8) as u8,
            6 => (ch.dad >> 16) as u8,
            7 => (ch.dad >> 24) as u8,
            8 => (ch.count & 0xFF) as u8,
            9 => (ch.count >> 8) as u8,
            10 => (ch.control & 0xFF) as u8,
            11 => (ch.control >> 8) as u8,
            _ => unreachable!(),
        }
    }

    pub(crate) fn write_register(&mut self, offset: usize, value: u8) {
        let ch = &mut self.channels[offset / 12];
        match offset % 12 {
            0 => ch.sad = (ch.sad & 0xFFFF_FF00) | value as u32,
            1 => ch.sad = (ch.sad & 0xFFFF_00FF) | ((value as u32) << 8),
            2 => ch.sad = (ch.sad & 0xFF00_FFFF) | ((value as u32) << 16),
            3 => ch.sad = (ch.sad & 0x00FF_FFFF) | ((value as u32) << 24),
            4 => ch.dad = (ch.dad & 0xFFFF_FF00) | value as u32,
            5 => ch.dad = (ch.dad & 0xFFFF_00FF) | ((value as u32) << 8),
            6 => ch.dad = (ch.dad & 0xFF00_FFFF) | ((value as u32) << 16),
            7 => ch.dad = (ch.dad & 0x00FF_FFFF) | ((value as u32) << 24),
            8 => ch.count = (ch.count & 0xFF00) | value as u16,
            9 => ch.count = (ch.count & 0x00FF) | ((value as u16) << 8),
            10 => ch.control = (ch.control & 0xFF00) | value as u16,
            11 => ch.control = (ch.control & 0x00FF) | ((value as u16) << 8),
            _ => unreachable!(),
        }
    }

    /// Whether the byte offset just written was DMAxCNT_H's high byte —
    /// the only byte whose write can newly arm a channel — paired with
    /// [`Self::armed_for_immediate`] to detect "just armed for immediate
    /// start" right after a register write, without the caller needing to
    /// know this module's internal byte layout.
    pub(crate) fn touched_control_high_byte(offset: usize) -> Option<usize> {
        if offset % 12 == 11 {
            Some(offset / 12)
        } else {
            None
        }
    }

    pub(crate) fn armed_for_immediate(&self, channel: usize) -> bool {
        self.channels[channel].enabled() && self.channels[channel].start_timing() == 0
    }

    /// Every channel currently enabled and armed for `timing`
    /// (1=VBlank, 2=HBlank) — checked once per PPU event by
    /// [`crate::memory::Bus::trigger_dma_for_timing`].
    pub(crate) fn channels_armed_for(&self, timing: u8) -> [bool; 4] {
        let mut out = [false; 4];
        for (i, ch) in self.channels.iter().enumerate() {
            out[i] = ch.enabled() && ch.start_timing() == timing;
        }
        out
    }

    /// Reads out everything [`crate::memory::Bus::execute_dma_transfer`]
    /// needs to actually perform channel `index`'s transfer.
    pub(crate) fn latch_for_transfer(&self, index: usize) -> DmaTransfer {
        let ch = &self.channels[index];
        DmaTransfer {
            src: ch.sad,
            dst: ch.dad,
            count: ch.effective_count(index),
            word32: ch.word32(),
            src_control: ch.src_control(),
            dest_control: ch.dest_control(),
            irq_enable: ch.irq_enable(),
        }
    }

    /// Called after a transfer completes: updates the source/dest
    /// registers to resume correctly on a future repeat, and clears the
    /// enable bit unless the channel repeats.
    pub(crate) fn finish_transfer(&mut self, index: usize, final_src: u32, final_dst: u32) {
        let ch = &mut self.channels[index];
        let keep_original_dad = ch.dest_control() == AddrControl::IncrementReload;
        let original_dad = ch.dad;
        ch.sad = final_src;
        ch.dad = if keep_original_dad { original_dad } else { final_dst };
        if !ch.repeat() {
            ch.disable();
        }
    }
}

/// Everything needed to actually execute one DMA transfer, read out of a
/// [`Dma`] channel at the moment it fires.
pub(crate) struct DmaTransfer {
    pub src: u32,
    pub dst: u32,
    pub count: u32,
    pub word32: bool,
    pub src_control: AddrControl,
    pub dest_control: AddrControl,
    pub irq_enable: bool,
}

/// Applies one channel's address-control mode after advancing by one
/// transfer unit (2 or 4 bytes, depending on the channel's word/halfword
/// setting).
pub(crate) fn step_addr(addr: u32, control: AddrControl, unit: u32) -> u32 {
    match control {
        AddrControl::Increment | AddrControl::IncrementReload => addr.wrapping_add(unit),
        AddrControl::Decrement => addr.wrapping_sub(unit),
        AddrControl::Fixed => addr,
    }
}

impl Default for Dma {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_channel(dma: &mut Dma, ch: usize, sad: u32, dad: u32, count: u16, control: u16) {
        let base = ch * 12;
        for i in 0..4 {
            dma.write_register(base + i, ((sad >> (i * 8)) & 0xFF) as u8);
        }
        for i in 0..4 {
            dma.write_register(base + 4 + i, ((dad >> (i * 8)) & 0xFF) as u8);
        }
        dma.write_register(base + 8, (count & 0xFF) as u8);
        dma.write_register(base + 9, (count >> 8) as u8);
        dma.write_register(base + 10, (control & 0xFF) as u8);
        dma.write_register(base + 11, (control >> 8) as u8);
    }

    #[test]
    fn registers_round_trip() {
        let mut dma = Dma::new();
        set_channel(&mut dma, 1, 0x0800_1234, 0x0600_0000, 0x0010, 0xB600);
        assert_eq!(dma.channels[1].sad, 0x0800_1234);
        assert_eq!(dma.channels[1].dad, 0x0600_0000);
        assert_eq!(dma.channels[1].count, 0x0010);
        assert_eq!(dma.channels[1].control, 0xB600);
    }

    #[test]
    fn armed_for_immediate_requires_enable_and_timing_zero() {
        let mut dma = Dma::new();
        set_channel(&mut dma, 3, 0, 0, 4, 0x8000); // enable, timing=0 (immediate)
        assert!(dma.armed_for_immediate(3));

        let mut dma2 = Dma::new();
        set_channel(&mut dma2, 3, 0, 0, 4, 0x9000); // enable, timing=1 (VBlank)
        assert!(!dma2.armed_for_immediate(3));
    }

    #[test]
    fn channels_armed_for_vblank_matches_start_timing_bits() {
        let mut dma = Dma::new();
        set_channel(&mut dma, 0, 0, 0, 4, 0x9000); // enable, VBlank
        set_channel(&mut dma, 2, 0, 0, 4, 0xA000); // enable, HBlank
        assert_eq!(dma.channels_armed_for(1), [true, false, false, false]);
        assert_eq!(dma.channels_armed_for(2), [false, false, true, false]);
    }

    #[test]
    fn effective_count_zero_means_max_and_respects_channel_width() {
        let mut dma = Dma::new();
        set_channel(&mut dma, 0, 0, 0, 0, 0x8000); // DMA0-2: 0 -> 0x4000
        assert_eq!(dma.channels[0].effective_count(0), 0x4000);

        set_channel(&mut dma, 3, 0, 0, 0, 0x8000); // DMA3: 0 -> 0x10000
        assert_eq!(dma.channels[3].effective_count(3), 0x1_0000);
    }

    #[test]
    fn finish_transfer_clears_enable_unless_repeating() {
        let mut dma = Dma::new();
        set_channel(&mut dma, 0, 0, 0, 4, 0x8000); // enable, no repeat
        dma.finish_transfer(0, 0x10, 0x20);
        assert!(!dma.channels[0].enabled());
        assert_eq!(dma.channels[0].sad, 0x10);
        assert_eq!(dma.channels[0].dad, 0x20);

        set_channel(&mut dma, 1, 0, 0, 4, 0x8200); // enable + repeat
        dma.finish_transfer(1, 0x10, 0x20);
        assert!(dma.channels[1].enabled(), "repeat must leave the channel armed");
    }

    #[test]
    fn touched_control_high_byte_only_matches_that_exact_offset() {
        assert_eq!(Dma::touched_control_high_byte(11), Some(0));
        assert_eq!(Dma::touched_control_high_byte(23), Some(1));
        assert_eq!(Dma::touched_control_high_byte(10), None);
        assert_eq!(Dma::touched_control_high_byte(0), None);
    }
}
