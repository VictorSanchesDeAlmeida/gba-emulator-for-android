mod flash;
mod none;
mod sram;

pub use flash::Flash;
pub use none::NoSave;
pub use sram::Sram;

/// Size of a Flash backup chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashSize {
    Kb64,
    Kb128,
}

impl FlashSize {
    pub fn bytes(self) -> usize {
        match self {
            FlashSize::Kb64 => 64 * 1024,
            FlashSize::Kb128 => 128 * 1024,
        }
    }
}

/// The kind of backup/save memory a cartridge exposes, as identified by the
/// ASCII signature the linker embeds in the ROM (see [`detect_save_type`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveType {
    /// No backup memory signature found in the ROM.
    None,
    /// 32KB battery-backed SRAM, mapped flat at 0x0E000000.
    Sram32K,
    /// Flash chip behind the AA/55 command protocol (GBATEK "Backup Flash Memory").
    Flash(FlashSize),
    /// Serial EEPROM. The signature alone cannot distinguish the 512-byte
    /// (6-bit address) chip from the 8KB (14-bit address) chip — real
    /// hardware infers this from the DMA transfer length used to access it.
    /// Left undetermined here; resolved once the DMA controller exists.
    EepromUnknownSize,
}

/// Read/write access to a cartridge's backup memory.
///
/// Distinct implementations exist because SRAM, Flash and EEPROM chips have
/// completely different access protocols on real hardware — SRAM is a flat
/// byte array, Flash requires an AA/55 command sequence, EEPROM uses a
/// bit-serial protocol driven by DMA. Keeping this behind a trait lets
/// `Cartridge` stay agnostic of which chip a given game shipped with.
pub trait SaveBackend: std::fmt::Debug {
    fn save_type(&self) -> SaveType;
    fn read(&self, offset: usize) -> u8;
    fn write(&mut self, offset: usize, value: u8);

    /// Raw contents, for persisting to a `.sav` file. Order matches `read`.
    fn as_bytes(&self) -> &[u8];

    /// Restores contents previously obtained from [`Self::as_bytes`].
    /// Data shorter than the backend's capacity leaves the remainder
    /// untouched (e.g. loading a save from before a capacity change);
    /// longer data is truncated.
    fn load_bytes(&mut self, data: &[u8]);
}

const SIGNATURES: &[(&[u8], SaveType)] = &[
    (b"EEPROM_V", SaveType::EepromUnknownSize),
    (b"SRAM_V", SaveType::Sram32K),
    // "SRAM_F_V": a variant signature some games (e.g. Fire Emblem) ship
    // instead of plain "SRAM_V" — same 32KB flat battery-backed chip,
    // just a different string some linkers/tools embed.
    (b"SRAM_F_V", SaveType::Sram32K),
    (b"FLASH1M_V", SaveType::Flash(FlashSize::Kb128)),
    (b"FLASH512_V", SaveType::Flash(FlashSize::Kb64)),
    (b"FLASH_V", SaveType::Flash(FlashSize::Kb64)),
];

/// Scans a ROM image for the ASCII backup-type signature the official
/// linker (and most homebrew toolchains) embeds verbatim in the binary,
/// e.g. `"FLASH1M_V110"`. This is the same heuristic real-world emulators
/// use, since GBA cartridges expose no hardware register that reports
/// which backup chip is present.
pub fn detect_save_type(rom: &[u8]) -> SaveType {
    for (signature, save_type) in SIGNATURES {
        if rom.windows(signature.len()).any(|window| window == *signature) {
            return *save_type;
        }
    }
    SaveType::None
}

/// Builds the backend matching a [`SaveType`]. `EepromUnknownSize` has no
/// backend yet — EEPROM's bit-serial protocol is driven by DMA transfer
/// length, which doesn't exist until the DMA controller is implemented.
pub fn build_backend(save_type: SaveType) -> Option<Box<dyn SaveBackend>> {
    match save_type {
        SaveType::None => Some(Box::new(NoSave)),
        SaveType::Sram32K => Some(Box::new(Sram::new())),
        SaveType::Flash(size) => Some(Box::new(Flash::new(size))),
        SaveType::EepromUnknownSize => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_each_signature() {
        assert_eq!(detect_save_type(b"...SRAM_V113..."), SaveType::Sram32K);
        assert_eq!(
            detect_save_type(b"...SRAM_F_V102..."),
            SaveType::Sram32K,
            "the SRAM_F_V variant (e.g. Fire Emblem) is the same flat 32KB chip as SRAM_V"
        );
        assert_eq!(
            detect_save_type(b"...FLASH1M_V110..."),
            SaveType::Flash(FlashSize::Kb128)
        );
        assert_eq!(
            detect_save_type(b"...FLASH512_V130..."),
            SaveType::Flash(FlashSize::Kb64)
        );
        assert_eq!(
            detect_save_type(b"...FLASH_V120..."),
            SaveType::Flash(FlashSize::Kb64)
        );
        assert_eq!(
            detect_save_type(b"...EEPROM_V111..."),
            SaveType::EepromUnknownSize
        );
    }

    #[test]
    fn no_signature_means_no_save() {
        assert_eq!(detect_save_type(b"just some ordinary bytes"), SaveType::None);
    }

    #[test]
    fn eeprom_has_no_backend_yet() {
        assert!(build_backend(SaveType::EepromUnknownSize).is_none());
    }
}
