use super::{SaveBackend, SaveType};

const SRAM_SIZE: usize = 32 * 1024;

/// 32KB battery-backed SRAM. Flat byte array, no command protocol — every
/// address in range is directly readable/writable, mirrored every 32KB
/// across the 0x0E000000 backup region (mirroring is the Memory Bus's job,
/// not this backend's).
#[derive(Debug)]
pub struct Sram {
    bytes: Vec<u8>,
}

impl Sram {
    pub fn new() -> Self {
        Sram {
            bytes: vec![0xFF; SRAM_SIZE],
        }
    }
}

impl Default for Sram {
    fn default() -> Self {
        Self::new()
    }
}

impl SaveBackend for Sram {
    fn save_type(&self) -> SaveType {
        SaveType::Sram32K
    }

    fn read(&self, offset: usize) -> u8 {
        self.bytes[offset % SRAM_SIZE]
    }

    fn write(&mut self, offset: usize, value: u8) {
        let len = self.bytes.len();
        self.bytes[offset % len] = value;
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn load_bytes(&mut self, data: &[u8]) {
        let n = data.len().min(self.bytes.len());
        self.bytes[..n].copy_from_slice(&data[..n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_erased() {
        let sram = Sram::new();
        assert_eq!(sram.read(0), 0xFF);
        assert_eq!(sram.read(SRAM_SIZE - 1), 0xFF);
    }

    #[test]
    fn read_write_roundtrip() {
        let mut sram = Sram::new();
        sram.write(0x100, 0x42);
        assert_eq!(sram.read(0x100), 0x42);
    }

    #[test]
    fn wraps_at_32kb() {
        let mut sram = Sram::new();
        sram.write(0, 0x7A);
        assert_eq!(sram.read(SRAM_SIZE), 0x7A);
    }

    #[test]
    fn load_bytes_restores_a_save() {
        let mut sram = Sram::new();
        let mut saved = vec![0u8; SRAM_SIZE];
        saved[10] = 0x99;
        sram.load_bytes(&saved);
        assert_eq!(sram.read(10), 0x99);
    }
}
