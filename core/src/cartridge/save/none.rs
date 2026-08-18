use super::{SaveBackend, SaveType};

/// Backend for cartridges with no backup memory signature at all.
/// Reads return open-bus-ish 0xFF; writes are discarded.
#[derive(Debug, Default)]
pub struct NoSave;

impl SaveBackend for NoSave {
    fn save_type(&self) -> SaveType {
        SaveType::None
    }

    fn read(&self, _offset: usize) -> u8 {
        0xFF
    }

    fn write(&mut self, _offset: usize, _value: u8) {}

    fn as_bytes(&self) -> &[u8] {
        &[]
    }

    fn load_bytes(&mut self, _data: &[u8]) {}
}
