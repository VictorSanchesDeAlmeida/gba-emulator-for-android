//! The GBA's 4 hardware timers (TM0-TM3) at 0x04000100+4*n: TMxCNT_L (the
//! reload value on write, the live counter on read) and TMxCNT_H (prescaler
//! select, cascade mode, IRQ enable, start/stop). Each timer's counter
//! increments every N CPU cycles (N = 1/64/256/1024, picked by TMxCNT_H
//! bits 0-1) and reloads from TMxCNT_L's last-written value on overflow.
//! Timers 1-3 can instead run in "cascade" mode (TMxCNT_H bit 2): rather
//! than using their own prescaler, they increment once per overflow of the
//! *previous* timer, letting games chain e.g. TM0+TM1 into an effective
//! 32-bit counter.

pub const IO_BASE: usize = 0x100;
pub(crate) const IO_SIZE: usize = 16; // 4 timers * 4 bytes each

const PRESCALER_CYCLES: [u32; 4] = [1, 64, 256, 1024];

#[derive(Debug, Clone, Copy, Default)]
pub struct TimerIrqEvents {
    overflowed: [bool; 4],
}

impl TimerIrqEvents {
    pub fn overflowed(&self, index: usize) -> bool {
        self.overflowed[index]
    }
}

#[derive(Debug)]
struct TimerChannel {
    reload: u16,
    counter: u16,
    control: u16,
    subcycles: u32,
}

impl TimerChannel {
    fn new() -> Self {
        TimerChannel { reload: 0, counter: 0, control: 0, subcycles: 0 }
    }

    fn enabled(&self) -> bool {
        self.control & 0x80 != 0
    }
    fn cascade(&self) -> bool {
        self.control & 0x04 != 0
    }
    fn irq_enable(&self) -> bool {
        self.control & 0x40 != 0
    }
    fn prescaler_cycles(&self) -> u32 {
        PRESCALER_CYCLES[(self.control & 0x3) as usize]
    }

    /// Advances by direct CPU cycles through the prescaler. No-op in
    /// cascade mode, where the timer instead advances via
    /// [`Self::tick_cascade`]. Returns how many times it overflowed.
    fn tick_cycles(&mut self, cycles: u32) -> u32 {
        if !self.enabled() || self.cascade() {
            return 0;
        }
        self.subcycles += cycles;
        let period = self.prescaler_cycles();
        let mut overflow_count = 0;
        while self.subcycles >= period {
            self.subcycles -= period;
            overflow_count += self.step_counter();
        }
        overflow_count
    }

    /// Advances by `pulses` (the previous timer's overflow count this
    /// tick). No-op unless this timer is both enabled and in cascade mode.
    fn tick_cascade(&mut self, pulses: u32) -> u32 {
        if !self.enabled() || !self.cascade() {
            return 0;
        }
        let mut overflow_count = 0;
        for _ in 0..pulses {
            overflow_count += self.step_counter();
        }
        overflow_count
    }

    fn step_counter(&mut self) -> u32 {
        let (next, overflowed) = self.counter.overflowing_add(1);
        if overflowed {
            self.counter = self.reload;
            1
        } else {
            self.counter = next;
            0
        }
    }
}

impl Default for TimerChannel {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct Timers {
    channels: [TimerChannel; 4],
}

impl Timers {
    pub fn new() -> Self {
        Timers {
            channels: [TimerChannel::new(), TimerChannel::new(), TimerChannel::new(), TimerChannel::new()],
        }
    }

    /// Advances every timer by `cycles` CPU cycles, chaining cascade-mode
    /// channels off the previous channel's overflow count in the same
    /// call. Channel 0 can never cascade (there's no channel -1).
    pub fn tick(&mut self, cycles: u32) -> TimerIrqEvents {
        let mut events = TimerIrqEvents::default();
        let mut cascade_pulses = 0u32;
        for i in 0..4 {
            let overflow_count = if i == 0 {
                self.channels[0].tick_cycles(cycles)
            } else if self.channels[i].cascade() {
                self.channels[i].tick_cascade(cascade_pulses)
            } else {
                self.channels[i].tick_cycles(cycles)
            };
            if overflow_count > 0 && self.channels[i].irq_enable() {
                events.overflowed[i] = true;
            }
            cascade_pulses = overflow_count;
        }
        events
    }

    pub(crate) fn read_register(&self, offset: usize) -> u8 {
        let idx = offset / 4;
        let ch = &self.channels[idx];
        match offset % 4 {
            0 => (ch.counter & 0xFF) as u8,
            1 => (ch.counter >> 8) as u8,
            2 => (ch.control & 0xFF) as u8,
            3 => (ch.control >> 8) as u8,
            _ => unreachable!(),
        }
    }

    pub(crate) fn write_register(&mut self, offset: usize, value: u8) {
        let idx = offset / 4;
        let ch = &mut self.channels[idx];
        match offset % 4 {
            0 => ch.reload = (ch.reload & 0xFF00) | value as u16,
            1 => ch.reload = (ch.reload & 0x00FF) | ((value as u16) << 8),
            2 => {
                let was_enabled = ch.enabled();
                ch.control = (ch.control & 0xFF00) | value as u16;
                // Real hardware: an 0->1 transition on the start bit loads
                // the counter from the reload value immediately, rather
                // than waiting for the first overflow.
                if !was_enabled && ch.enabled() {
                    ch.counter = ch.reload;
                    ch.subcycles = 0;
                }
            }
            3 => ch.control = (ch.control & 0x00FF) | ((value as u16) << 8),
            _ => unreachable!(),
        }
    }
}

impl Default for Timers {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enable_timer(timers: &mut Timers, index: usize, prescaler_select: u8, irq: bool, cascade: bool) {
        let base = index * 4;
        let mut ctrl = prescaler_select & 0x3;
        if cascade {
            ctrl |= 0x04;
        }
        if irq {
            ctrl |= 0x40;
        }
        ctrl |= 0x80; // enable
        timers.write_register(base + 2, ctrl);
    }

    fn set_reload(timers: &mut Timers, index: usize, value: u16) {
        let base = index * 4;
        timers.write_register(base, (value & 0xFF) as u8);
        timers.write_register(base + 1, (value >> 8) as u8);
    }

    #[test]
    fn disabled_timer_does_not_count() {
        let mut timers = Timers::new();
        timers.tick(10_000);
        assert_eq!(timers.read_register(0), 0);
        assert_eq!(timers.read_register(1), 0);
    }

    #[test]
    fn prescaler_1_counts_one_per_cycle() {
        let mut timers = Timers::new();
        enable_timer(&mut timers, 0, 0, false, false);
        timers.tick(5);
        assert_eq!(timers.read_register(0), 5);
    }

    #[test]
    fn prescaler_64_counts_once_every_64_cycles() {
        let mut timers = Timers::new();
        enable_timer(&mut timers, 0, 1, false, false);
        timers.tick(63);
        assert_eq!(timers.read_register(0), 0, "not enough cycles yet");
        timers.tick(1);
        assert_eq!(timers.read_register(0), 1);
    }

    #[test]
    fn enabling_loads_the_counter_from_reload_immediately() {
        let mut timers = Timers::new();
        set_reload(&mut timers, 0, 0xFFF0);
        enable_timer(&mut timers, 0, 0, false, false);
        assert_eq!(timers.read_register(0), 0xF0);
        assert_eq!(timers.read_register(1), 0xFF);
    }

    #[test]
    fn overflow_reloads_and_reports_irq_when_enabled() {
        let mut timers = Timers::new();
        set_reload(&mut timers, 0, 0xFFFE);
        enable_timer(&mut timers, 0, 0, true, false);

        let events = timers.tick(2);
        assert_eq!(timers.read_register(0), 0xFE);
        assert_eq!(timers.read_register(1), 0xFF);
        assert!(events.overflowed(0));
    }

    #[test]
    fn overflow_without_irq_enable_does_not_report() {
        let mut timers = Timers::new();
        set_reload(&mut timers, 0, 0xFFFF);
        enable_timer(&mut timers, 0, 0, false, false);

        let events = timers.tick(1);
        assert!(!events.overflowed(0));
    }

    #[test]
    fn cascade_timer_ignores_its_own_prescaler_and_counts_previous_overflows() {
        let mut timers = Timers::new();
        set_reload(&mut timers, 0, 0xFFFF); // overflows on the very next tick
        enable_timer(&mut timers, 0, 0, false, false);
        enable_timer(&mut timers, 1, 0, false, true); // cascade

        timers.tick(1); // timer0 overflows once
        assert_eq!(timers.read_register(4), 1, "timer1 counted one cascade pulse");

        // Disable timer0 so it stops producing cascade pulses, then hammer
        // timer1 with a huge direct-cycle tick: a cascade timer must not
        // advance via its own prescaler, only via the previous timer's
        // overflow count.
        timers.write_register(2, 0x00);
        timers.tick(1_000_000);
        assert_eq!(timers.read_register(4), 1, "timer1 must ignore direct cycles while cascading");
    }

    #[test]
    fn cascade_chain_can_itself_overflow_and_request_irq() {
        let mut timers = Timers::new();
        set_reload(&mut timers, 0, 0xFFFF);
        enable_timer(&mut timers, 0, 0, false, false);
        set_reload(&mut timers, 1, 0xFFFF);
        enable_timer(&mut timers, 1, 0, true, true);

        let events = timers.tick(1);
        assert!(events.overflowed(1), "timer1 must overflow the same tick it receives its one cascade pulse");
    }
}
