//! GBA Direct Sound (DMA Sound) channels A/B: a 32-byte FIFO of signed
//! 8-bit PCM samples, refilled by the CPU (or, on real hardware, DMA1/2 —
//! not yet modeled, since this project has no general DMA controller)
//! writing to FIFO_A/FIFO_B, and drained one byte at a time whenever the
//! timer selected by SOUNDCNT_H overflows.

const FIFO_CAPACITY: usize = 32;

#[derive(Debug)]
pub(crate) struct DmaSoundChannel {
    fifo: std::collections::VecDeque<i8>,
    current_sample: i8,
}

impl DmaSoundChannel {
    pub(crate) fn new() -> Self {
        DmaSoundChannel { fifo: std::collections::VecDeque::with_capacity(FIFO_CAPACITY), current_sample: 0 }
    }

    /// Pushes one byte into the FIFO (from a byte/halfword/word write to
    /// the FIFO_A/FIFO_B register). Real hardware silently drops writes
    /// once the 32-byte FIFO is full.
    pub(crate) fn push(&mut self, value: u8) {
        if self.fifo.len() < FIFO_CAPACITY {
            self.fifo.push_back(value as i8);
        }
    }

    /// Clears the FIFO (SOUNDCNT_H's per-channel "reset" bit).
    pub(crate) fn reset(&mut self) {
        self.fifo.clear();
        self.current_sample = 0;
    }

    /// Pops the next sample into `current_sample`, called when the
    /// channel's configured timer overflows. Holds the last sample instead
    /// of going silent if the FIFO underruns (matches real hardware).
    pub(crate) fn pop(&mut self) {
        if let Some(sample) = self.fifo.pop_front() {
            self.current_sample = sample;
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.fifo.len()
    }

    /// The signed 8-bit sample currently latched for output.
    pub(crate) fn sample(&self) -> i8 {
        self.current_sample
    }
}

impl Default for DmaSoundChannel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_pop_are_fifo_ordered() {
        let mut ch = DmaSoundChannel::new();
        ch.push(10);
        ch.push(20);
        ch.pop();
        assert_eq!(ch.sample(), 10);
        ch.pop();
        assert_eq!(ch.sample(), 20);
    }

    #[test]
    fn signed_bytes_round_trip_correctly() {
        let mut ch = DmaSoundChannel::new();
        ch.push(0x80); // -128 as i8
        ch.pop();
        assert_eq!(ch.sample(), -128);
    }

    #[test]
    fn pushes_beyond_capacity_are_dropped() {
        let mut ch = DmaSoundChannel::new();
        for i in 0..40u8 {
            ch.push(i);
        }
        assert_eq!(ch.len(), 32);
    }

    #[test]
    fn popping_an_empty_fifo_holds_the_last_sample() {
        let mut ch = DmaSoundChannel::new();
        ch.push(42);
        ch.pop();
        assert_eq!(ch.sample(), 42);
        ch.pop(); // FIFO now empty
        assert_eq!(ch.sample(), 42, "must not go silent on underrun");
    }

    #[test]
    fn reset_clears_the_fifo_and_current_sample() {
        let mut ch = DmaSoundChannel::new();
        ch.push(5);
        ch.pop();
        ch.reset();
        assert_eq!(ch.len(), 0);
        assert_eq!(ch.sample(), 0);
    }
}
