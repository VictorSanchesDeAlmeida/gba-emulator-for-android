mod bus;

pub use bus::Bus;

/// Byte/halfword/word access to the address bus. Exists as a trait (rather
/// than making every consumer depend on the concrete [`Bus`]) so the CPU can
/// be unit-tested against a small in-memory fake instead of a full
/// `Bus` + `Cartridge`.
pub trait Memory {
    fn read8(&self, addr: u32) -> u8;
    fn write8(&mut self, addr: u32, value: u8);
    fn read16(&self, addr: u32) -> u16;
    fn write16(&mut self, addr: u32, value: u16);
    fn read32(&self, addr: u32) -> u32;
    fn write32(&mut self, addr: u32, value: u32);
}

impl Memory for Bus {
    fn read8(&self, addr: u32) -> u8 {
        Bus::read8(self, addr)
    }

    fn write8(&mut self, addr: u32, value: u8) {
        Bus::write8(self, addr, value);
    }

    fn read16(&self, addr: u32) -> u16 {
        Bus::read16(self, addr)
    }

    fn write16(&mut self, addr: u32, value: u16) {
        Bus::write16(self, addr, value);
    }

    fn read32(&self, addr: u32) -> u32 {
        Bus::read32(self, addr)
    }

    fn write32(&mut self, addr: u32, value: u32) {
        Bus::write32(self, addr, value);
    }
}
