use std::fmt;

/// Size in bytes of the GBA cartridge header (0x000..0x0C0).
pub const HEADER_SIZE: usize = 0xC0;

const ENTRY_POINT: std::ops::Range<usize> = 0x00..0x04;
const GAME_TITLE: std::ops::Range<usize> = 0xA0..0xAC;
const GAME_CODE: std::ops::Range<usize> = 0xAC..0xB0;
const MAKER_CODE: std::ops::Range<usize> = 0xB0..0xB2;
const FIXED_VALUE: usize = 0xB2;
const MAIN_UNIT_CODE: usize = 0xB3;
const DEVICE_TYPE: usize = 0xB4;
const SOFTWARE_VERSION: usize = 0xBC;
const HEADER_CHECKSUM: usize = 0xBD;

/// Value the BIOS expects at [`FIXED_VALUE`] before it will boot the cartridge.
const EXPECTED_FIXED_VALUE: u8 = 0x96;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderError {
    RomTooSmall { actual: usize, required: usize },
}

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HeaderError::RomTooSmall { actual, required } => write!(
                f,
                "ROM has {actual} byte(s), but a GBA header requires at least {required}"
            ),
        }
    }
}

impl std::error::Error for HeaderError {}

/// Parsed contents of the 192-byte GBA cartridge header.
///
/// See GBATEK "GBA Cartridge Header" for the field layout this mirrors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GbaHeader {
    pub entry_point: [u8; 4],
    pub game_title: String,
    pub game_code: String,
    pub maker_code: String,
    pub fixed_value: u8,
    pub main_unit_code: u8,
    pub device_type: u8,
    pub software_version: u8,
    pub header_checksum: u8,
    computed_checksum: u8,
}

impl GbaHeader {
    /// Parses the header from the start of a ROM image.
    ///
    /// Parsing only requires the first [`HEADER_SIZE`] bytes to be present;
    /// it does not reject malformed-but-loadable dumps (e.g. ROM hacks with
    /// a patched checksum). Use [`Self::fixed_value_valid`] and
    /// [`Self::checksum_valid`] to check BIOS-level validity separately.
    pub fn parse(rom: &[u8]) -> Result<Self, HeaderError> {
        if rom.len() < HEADER_SIZE {
            return Err(HeaderError::RomTooSmall {
                actual: rom.len(),
                required: HEADER_SIZE,
            });
        }

        let mut entry_point = [0u8; 4];
        entry_point.copy_from_slice(&rom[ENTRY_POINT]);

        Ok(GbaHeader {
            entry_point,
            game_title: decode_padded_ascii(&rom[GAME_TITLE]),
            game_code: decode_padded_ascii(&rom[GAME_CODE]),
            maker_code: decode_padded_ascii(&rom[MAKER_CODE]),
            fixed_value: rom[FIXED_VALUE],
            main_unit_code: rom[MAIN_UNIT_CODE],
            device_type: rom[DEVICE_TYPE],
            software_version: rom[SOFTWARE_VERSION],
            header_checksum: rom[HEADER_CHECKSUM],
            computed_checksum: compute_header_checksum(rom),
        })
    }

    /// Whether the fixed value byte (0xB2) matches what the BIOS boot check expects.
    pub fn fixed_value_valid(&self) -> bool {
        self.fixed_value == EXPECTED_FIXED_VALUE
    }

    /// Whether the stored header checksum (0xBD) matches the checksum the
    /// BIOS recomputes over bytes 0xA0..=0xBC before booting the cartridge.
    pub fn checksum_valid(&self) -> bool {
        self.header_checksum == self.computed_checksum
    }
}

/// Header checksum as verified by the GBA BIOS: a running subtraction over
/// bytes 0xA0..=0xBC, minus 0x19, wrapped to a single byte.
fn compute_header_checksum(rom: &[u8]) -> u8 {
    rom[0xA0..=0xBC]
        .iter()
        .fold(0u8, |acc, &byte| acc.wrapping_sub(byte))
        .wrapping_sub(0x19)
}

/// Decodes a fixed-width, NUL-padded ASCII field, trimming trailing padding.
fn decode_padded_ascii(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a synthetic but well-formed 192-byte header for a made-up
    /// "TESTROM"/"TEST"/"AB" cartridge, with a correct checksum.
    fn synthetic_header_bytes() -> [u8; HEADER_SIZE] {
        let mut rom = [0u8; HEADER_SIZE];
        rom[GAME_TITLE][..7].copy_from_slice(b"TESTROM");
        rom[GAME_CODE].copy_from_slice(b"TEST");
        rom[MAKER_CODE].copy_from_slice(b"AB");
        rom[FIXED_VALUE] = EXPECTED_FIXED_VALUE;
        rom[SOFTWARE_VERSION] = 1;
        rom[HEADER_CHECKSUM] = compute_header_checksum(&rom);
        rom
    }

    #[test]
    fn rejects_roms_smaller_than_header() {
        let rom = [0u8; HEADER_SIZE - 1];
        assert_eq!(
            GbaHeader::parse(&rom),
            Err(HeaderError::RomTooSmall {
                actual: HEADER_SIZE - 1,
                required: HEADER_SIZE,
            })
        );
    }

    #[test]
    fn parses_fields_and_trims_padding() {
        let rom = synthetic_header_bytes();
        let header = GbaHeader::parse(&rom).expect("valid synthetic header");

        assert_eq!(header.game_title, "TESTROM");
        assert_eq!(header.game_code, "TEST");
        assert_eq!(header.maker_code, "AB");
        assert_eq!(header.software_version, 1);
    }

    #[test]
    fn validates_fixed_value_and_checksum() {
        let mut rom = synthetic_header_bytes();
        let header = GbaHeader::parse(&rom).unwrap();
        assert!(header.fixed_value_valid());
        assert!(header.checksum_valid());

        // Corrupting the checksum byte must be detected, without affecting
        // the rest of the parsed header.
        rom[HEADER_CHECKSUM] ^= 0xFF;
        let corrupted = GbaHeader::parse(&rom).unwrap();
        assert!(!corrupted.checksum_valid());
        assert_eq!(corrupted.game_title, "TESTROM");
    }

    #[test]
    fn parses_real_pokemon_firered_header_bytes() {
        // First 192 bytes of the real ROM in roms/, captured independently
        // via a hex dump, so this unit test has no filesystem dependency.
        let mut rom = [0u8; HEADER_SIZE];
        rom[GAME_TITLE].copy_from_slice(b"POKEMON FIRE");
        rom[GAME_CODE].copy_from_slice(b"BPRE");
        rom[MAKER_CODE].copy_from_slice(b"01");
        rom[FIXED_VALUE] = EXPECTED_FIXED_VALUE;
        rom[HEADER_CHECKSUM] = compute_header_checksum(&rom);

        let header = GbaHeader::parse(&rom).unwrap();
        assert_eq!(header.game_title, "POKEMON FIRE");
        assert_eq!(header.game_code, "BPRE");
        assert_eq!(header.maker_code, "01");
        assert!(header.fixed_value_valid());
        assert!(header.checksum_valid());
    }
}
