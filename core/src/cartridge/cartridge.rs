use std::fmt;

use super::header::{GbaHeader, HeaderError};
use super::save::{self, SaveBackend, SaveType};

#[derive(Debug)]
pub enum CartridgeError {
    Header(HeaderError),
    /// The detected save type has no backend implementation yet (currently
    /// only EEPROM, whose bit-serial protocol needs the DMA controller).
    UnsupportedSaveType(SaveType),
}

impl fmt::Display for CartridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CartridgeError::Header(err) => write!(f, "invalid cartridge header: {err}"),
            CartridgeError::UnsupportedSaveType(save_type) => {
                write!(f, "no backend implemented yet for save type {save_type:?}")
            }
        }
    }
}

impl std::error::Error for CartridgeError {}

/// A loaded GBA cartridge: ROM bytes, parsed header, and whichever backup
/// memory backend matches the save-type signature embedded in the ROM.
///
/// Unlike Game Boy cartridges, GBA ROM is flat-mapped with no bank-switching
/// registers to emulate — the only real per-cartridge variability is which
/// backup chip (if any) is present, which is why the polymorphism lives in
/// [`SaveBackend`] rather than in `Cartridge` itself.
#[derive(Debug)]
pub struct Cartridge {
    rom: Vec<u8>,
    header: GbaHeader,
    save: Box<dyn SaveBackend>,
}

impl Cartridge {
    pub fn load(rom: Vec<u8>) -> Result<Self, CartridgeError> {
        let header = GbaHeader::parse(&rom).map_err(CartridgeError::Header)?;
        let save_type = save::detect_save_type(&rom);
        let save = save::build_backend(save_type)
            .ok_or(CartridgeError::UnsupportedSaveType(save_type))?;

        Ok(Cartridge { rom, header, save })
    }

    pub fn header(&self) -> &GbaHeader {
        &self.header
    }

    pub fn save_type(&self) -> SaveType {
        self.save.save_type()
    }

    pub fn rom_size(&self) -> usize {
        self.rom.len()
    }

    /// Reads a byte at `offset` from the start of the ROM image.
    ///
    /// Real hardware returns "open bus" data for offsets past the end of a
    /// ROM smaller than its address window, derived from the full 32-bit
    /// bus address. `Cartridge` only sees an offset, not that address, so
    /// precise open-bus emulation belongs to the Memory Bus (Etapa 3); this
    /// returns 0 for now.
    pub fn read_rom_byte(&self, offset: usize) -> u8 {
        self.rom.get(offset).copied().unwrap_or(0)
    }

    pub fn read_save_byte(&self, offset: usize) -> u8 {
        self.save.read(offset)
    }

    pub fn write_save_byte(&mut self, offset: usize, value: u8) {
        self.save.write(offset, value);
    }

    /// Raw backup memory contents, for persisting to a `.sav` file.
    pub fn save_bytes(&self) -> &[u8] {
        self.save.as_bytes()
    }

    /// Restores backup memory previously obtained from [`Self::save_bytes`].
    pub fn load_save_bytes(&mut self, data: &[u8]) {
        self.save.load_bytes(data);
    }
}

#[cfg(test)]
mod tests {
    use super::super::save::FlashSize;
    use super::*;
    use crate::cartridge::header::HEADER_SIZE;

    fn rom_with_signature(signature: &[u8]) -> Vec<u8> {
        let mut rom = vec![0u8; HEADER_SIZE + 64];
        rom[0xB2] = 0x96; // fixed value
        rom[HEADER_SIZE..HEADER_SIZE + signature.len()].copy_from_slice(signature);
        rom
    }

    #[test]
    fn rejects_a_rom_too_small_for_a_header() {
        let err = Cartridge::load(vec![0u8; 10]).unwrap_err();
        assert!(matches!(err, CartridgeError::Header(_)));
    }

    #[test]
    fn detects_save_type_from_embedded_signature() {
        let cart = Cartridge::load(rom_with_signature(b"FLASH1M_V110")).unwrap();
        assert_eq!(cart.save_type(), SaveType::Flash(FlashSize::Kb128));
    }

    #[test]
    fn rom_without_signature_gets_no_save_backend() {
        let cart = Cartridge::load(rom_with_signature(b"nothing here")).unwrap();
        assert_eq!(cart.save_type(), SaveType::None);
    }

    #[test]
    fn eeprom_signature_is_rejected_until_dma_exists() {
        let err = Cartridge::load(rom_with_signature(b"EEPROM_V111")).unwrap_err();
        assert!(matches!(err, CartridgeError::UnsupportedSaveType(_)));
    }

    #[test]
    fn save_read_write_goes_through_the_backend() {
        let mut cart = Cartridge::load(rom_with_signature(b"SRAM_V113")).unwrap();
        cart.write_save_byte(5, 0xAB);
        assert_eq!(cart.read_save_byte(5), 0xAB);
    }
}
