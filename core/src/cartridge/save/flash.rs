use super::{FlashSize, SaveBackend, SaveType};

const SECTOR_SIZE: usize = 0x1000;
const BANK_SIZE: usize = 0x10000;

/// Command state machine of a GBA Flash backup chip (GBATEK "Backup Flash
/// Memory"). Every operation — programming a byte, erasing a sector, erasing
/// the whole chip, reading the manufacturer/device ID, switching banks on a
/// 128KB chip — is unlocked by writing the fixed byte sequence 0xAA to
/// 0x5555 then 0x55 to 0x2AAA, followed by a command byte at 0x5555.
///
/// Real chips take real time to erase/program and expose that via a toggle
/// bit on subsequent reads; this backend completes every operation
/// immediately. Games detect completion by polling for expected data rather
/// than a fixed delay, so instant completion is transparent to them.
#[derive(Debug)]
pub struct Flash {
    size: FlashSize,
    bytes: Vec<u8>,
    bank: usize,
    state: State,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ready,
    Unlocked1,
    Unlocked2,
    IdMode,
    ErasePrefixReady,
    ErasePrefixUnlocked1,
    ErasePrefixUnlocked2,
    PendingProgram,
    PendingBankSwitch,
}

impl Flash {
    pub fn new(size: FlashSize) -> Self {
        Flash {
            size,
            bytes: vec![0xFF; size.bytes()],
            bank: 0,
            state: State::Ready,
        }
    }

    fn manufacturer_id(&self) -> u8 {
        match self.size {
            FlashSize::Kb64 => 0xBF,  // SST
            FlashSize::Kb128 => 0x62, // Sanyo
        }
    }

    fn device_id(&self) -> u8 {
        match self.size {
            FlashSize::Kb64 => 0xD4,  // SST39VF512
            FlashSize::Kb128 => 0x13, // LE26FV10N1TS
        }
    }

    fn data_at(&self, addr: usize) -> u8 {
        self.bytes[self.bank * BANK_SIZE + addr]
    }

    fn program_byte(&mut self, addr: usize, value: u8) {
        // Real Flash programming can only clear bits (1 -> 0); erasing is
        // what resets them to 1. AND matches that even if a game programs
        // without erasing first.
        let index = self.bank * BANK_SIZE + addr;
        self.bytes[index] &= value;
    }

    fn erase_sector(&mut self, addr: usize) {
        let base = self.bank * BANK_SIZE + (addr & !(SECTOR_SIZE - 1));
        self.bytes[base..base + SECTOR_SIZE].fill(0xFF);
    }

    fn erase_chip(&mut self) {
        self.bytes.fill(0xFF);
    }
}

impl SaveBackend for Flash {
    fn save_type(&self) -> SaveType {
        SaveType::Flash(self.size)
    }

    fn read(&self, offset: usize) -> u8 {
        let addr = offset & 0xFFFF;
        match (self.state, addr) {
            (State::IdMode, 0x0000) => self.manufacturer_id(),
            (State::IdMode, 0x0001) => self.device_id(),
            _ => self.data_at(addr),
        }
    }

    fn write(&mut self, offset: usize, value: u8) {
        let addr = offset & 0xFFFF;
        self.state = match (self.state, addr, value) {
            (State::Ready | State::IdMode, 0x5555, 0xAA) => State::Unlocked1,
            (State::Ready | State::IdMode, _, _) => self.state,

            (State::Unlocked1, 0x2AAA, 0x55) => State::Unlocked2,
            (State::Unlocked1, _, _) => State::Ready,

            (State::Unlocked2, 0x5555, 0x90) => State::IdMode,
            (State::Unlocked2, 0x5555, 0xF0) => State::Ready,
            (State::Unlocked2, 0x5555, 0x80) => State::ErasePrefixReady,
            (State::Unlocked2, 0x5555, 0xA0) => State::PendingProgram,
            (State::Unlocked2, 0x5555, 0xB0) => State::PendingBankSwitch,
            (State::Unlocked2, _, _) => State::Ready,

            (State::ErasePrefixReady, 0x5555, 0xAA) => State::ErasePrefixUnlocked1,
            (State::ErasePrefixReady, _, _) => State::Ready,

            (State::ErasePrefixUnlocked1, 0x2AAA, 0x55) => State::ErasePrefixUnlocked2,
            (State::ErasePrefixUnlocked1, _, _) => State::Ready,

            (State::ErasePrefixUnlocked2, 0x5555, 0x10) => {
                self.erase_chip();
                State::Ready
            }
            (State::ErasePrefixUnlocked2, _, 0x30) => {
                self.erase_sector(addr);
                State::Ready
            }
            (State::ErasePrefixUnlocked2, _, _) => State::Ready,

            (State::PendingProgram, _, _) => {
                self.program_byte(addr, value);
                State::Ready
            }
            (State::PendingBankSwitch, _, _) => {
                if self.size == FlashSize::Kb128 {
                    self.bank = (value & 0x01) as usize;
                }
                State::Ready
            }
        };
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

    fn unlock(flash: &mut Flash) {
        flash.write(0x5555, 0xAA);
        flash.write(0x2AAA, 0x55);
    }

    #[test]
    fn starts_erased() {
        let flash = Flash::new(FlashSize::Kb64);
        assert_eq!(flash.read(0), 0xFF);
    }

    #[test]
    fn program_byte_requires_full_unlock_sequence() {
        let mut flash = Flash::new(FlashSize::Kb64);
        // No unlock sequence: stray writes must not program anything.
        flash.write(0x1234, 0x42);
        assert_eq!(flash.read(0x1234), 0xFF);

        unlock(&mut flash);
        flash.write(0x5555, 0xA0); // byte-program command
        flash.write(0x1234, 0x42); // data + address for the program
        assert_eq!(flash.read(0x1234), 0x42);
    }

    #[test]
    fn program_can_only_clear_bits() {
        let mut flash = Flash::new(FlashSize::Kb64);
        unlock(&mut flash);
        flash.write(0x5555, 0xA0);
        flash.write(0x10, 0b1100_1100);

        // Programming again without erasing ANDs with the existing byte.
        unlock(&mut flash);
        flash.write(0x5555, 0xA0);
        flash.write(0x10, 0b1111_0000);
        assert_eq!(flash.read(0x10), 0b1100_0000);
    }

    #[test]
    fn sector_erase_resets_only_that_sector() {
        let mut flash = Flash::new(FlashSize::Kb64);
        unlock(&mut flash);
        flash.write(0x5555, 0xA0);
        flash.write(0x10, 0x00);
        unlock(&mut flash);
        flash.write(0x5555, 0xA0);
        flash.write(SECTOR_SIZE + 0x10, 0x00);

        unlock(&mut flash);
        flash.write(0x5555, 0x80);
        unlock(&mut flash);
        flash.write(0x10, 0x30); // sector erase, sector 0

        assert_eq!(flash.read(0x10), 0xFF, "sector 0 should be erased");
        assert_eq!(
            flash.read(SECTOR_SIZE + 0x10),
            0x00,
            "sector 1 must be untouched"
        );
    }

    #[test]
    fn chip_erase_resets_everything() {
        let mut flash = Flash::new(FlashSize::Kb64);
        unlock(&mut flash);
        flash.write(0x5555, 0xA0);
        flash.write(0x10, 0x00);

        unlock(&mut flash);
        flash.write(0x5555, 0x80);
        unlock(&mut flash);
        flash.write(0x5555, 0x10); // chip erase

        assert_eq!(flash.read(0x10), 0xFF);
    }

    #[test]
    fn reads_manufacturer_and_device_id_in_id_mode() {
        let mut flash = Flash::new(FlashSize::Kb128);
        unlock(&mut flash);
        flash.write(0x5555, 0x90); // enter ID mode
        assert_eq!(flash.read(0x0000), 0x62);
        assert_eq!(flash.read(0x0001), 0x13);

        unlock(&mut flash);
        flash.write(0x5555, 0xF0); // exit ID mode
        assert_eq!(flash.read(0x0000), 0xFF, "back to normal data reads");
    }

    #[test]
    fn bank_switch_only_applies_to_128kb_chips() {
        let mut flash64 = Flash::new(FlashSize::Kb64);
        unlock(&mut flash64);
        flash64.write(0x5555, 0xB0);
        flash64.write(0x0000, 0x01); // ignored: 64KB chip has one bank
        unlock(&mut flash64);
        flash64.write(0x5555, 0xA0);
        flash64.write(0x10, 0x00);
        assert_eq!(flash64.read(0x10), 0x00);

        let mut flash128 = Flash::new(FlashSize::Kb128);
        unlock(&mut flash128);
        flash128.write(0x5555, 0xA0);
        flash128.write(0x10, 0x11); // bank 0

        unlock(&mut flash128);
        flash128.write(0x5555, 0xB0);
        flash128.write(0x0000, 0x01); // switch to bank 1
        unlock(&mut flash128);
        flash128.write(0x5555, 0xA0);
        flash128.write(0x10, 0x22); // bank 1

        assert_eq!(flash128.read(0x10), 0x22, "bank 1 active");

        unlock(&mut flash128);
        flash128.write(0x5555, 0xB0);
        flash128.write(0x0000, 0x00); // back to bank 0
        assert_eq!(flash128.read(0x10), 0x11, "bank 0 untouched by bank 1 write");
    }
}
