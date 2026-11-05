//! Carrier bursts whose end is owned by TIMER0, independent of task polling.
use bl706_hal::{
    gpio::Pin,
    pwm::Pwm,
    timer::{self, Compare, Events, Timer},
};
use embassy_time::{Duration, Instant};
use pinecil_rs::heater::{MAX_BURST_US, MIN_BURST_US};
use static_cell::StaticCell;

pub type Carrier = Pwm<1, Pin<21>>;
type Driver = timer::InterruptDriver<0, Output>;
static DRIVER: StaticCell<Driver> = StaticCell::new();

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub completed: u32,
    pub cancelled: u32,
    pub last_duration_us: u64,
    pub max_late_us: u64,
    pub active: bool,
}

pub struct Output {
    pwm: Carrier,
    stats: Stats,
    started_us: u64,
    requested_us: u32,
}
impl timer::Handler<0> for Output {
    fn on_interrupt(&mut self, timer: &mut Timer<0>, _events: Events) {
        // First action on every timer event. A fault never starts another burst.
        self.pwm.disable();
        timer.listen(Events::NONE);
        if self.stats.active {
            let elapsed = Instant::now().as_micros().saturating_sub(self.started_us);
            self.stats.last_duration_us = elapsed;
            self.stats.max_late_us = self
                .stats
                .max_late_us
                .max(elapsed.saturating_sub(u64::from(self.requested_us)));
            self.stats.completed = self.stats.completed.wrapping_add(1);
            self.stats.active = false;
        }
    }
}

#[derive(Clone, Copy)]
pub struct HeaterOutput(&'static Driver);
impl HeaterOutput {
    pub fn new(pwm: Carrier, timer: Timer<0>) -> Self {
        assert_eq!(timer.clock_hz(), 1_000_000);
        let driver = DRIVER.init(Driver::new(
            timer,
            Output {
                pwm,
                stats: Stats::default(),
                started_us: 0,
                requested_us: 0,
            },
        ));
        // Keep the free-running counter alive; absolute future compares work
        // across 32-bit wrap and do not depend on counter restart semantics.
        driver.register(Events::NONE);
        Self(driver)
    }

    pub fn stop(&self) -> Stats {
        self.0.with(|timer, output| {
            output.pwm.disable();
            timer.listen(Events::NONE);
            if output.stats.active {
                output.stats.cancelled = output.stats.cancelled.wrapping_add(1);
                output.stats.active = false;
            }
            output.stats
        })
    }

    pub fn stats(&self) -> Stats {
        self.0.with(|_, output| output.stats)
    }

    /// Each arm consumes a single bounded interval. No automatic repetition.
    /// `drive=false` exercises the counter/ISR with PWM forced to zero duty.
    pub fn burst(&self, duration_us: u32, drive: bool) -> Result<(), ()> {
        if !(MIN_BURST_US..=MAX_BURST_US).contains(&duration_us) {
            self.stop();
            return Err(());
        }
        self.0.with(|timer, output| {
            if output.stats.active {
                return Err(());
            }
            output.pwm.disable();
            output
                .pwm
                .set_duty_cycle(if drive { 50 } else { 0 })
                .map_err(|_| ())?;
            let deadline = timer.counter().wrapping_add(duration_us);
            timer.set_compare(Compare::C0, deadline).map_err(|_| ())?;
            timer.acknowledge(Events::compare(Compare::C0));
            output.started_us = Instant::now().as_micros();
            output.requested_us = duration_us;
            output.stats.active = true;
            timer.listen(Events::compare(Compare::C0));
            output.pwm.enable();
            Ok(())
        })
    }

    /// Startup checks leave GPIO21 at zero duty throughout. The second check
    /// deliberately withholds executor polling while the cutoff IRQ runs.
    pub fn self_test(&self) -> bool {
        let before = self.stats();
        if self.burst(20_000, false).is_err() {
            return false;
        }
        let end = Instant::now() + Duration::from_millis(30);
        while Instant::now() < end {
            core::hint::spin_loop();
        }
        let first = self.stats();
        log::info!(
            "Heater timer dry test: request=20000 us actual={} us completed={}",
            first.last_duration_us,
            first.completed
        );
        if first.active
            || first.completed != before.completed.wrapping_add(1)
            || !(19_900..=21_000).contains(&first.last_duration_us)
        {
            self.stop();
            return false;
        }
        if self.burst(2000, false).is_err() {
            return false;
        }
        let end = Instant::now() + Duration::from_millis(6);
        while Instant::now() < end {
            core::hint::spin_loop();
        }
        let second = self.stats();
        log::info!(
            "Heater timer stalled-task test: request=2000 us actual={} us active={}",
            second.last_duration_us,
            second.active
        );
        self.stop();
        !second.active
            && second.completed == first.completed.wrapping_add(1)
            && (1900..=3000).contains(&second.last_duration_us)
    }
}

/// Last-resort panic path. The runtime has masked interrupts; normal driver
/// ownership must not prevent shutting down PWM while a RefCell is borrowed.
pub fn emergency_stop() {
    // PWM base + channel array 0x20 + PWM1 stride 0x20 + CONFIG offset 0x10.
    // Clear SW_FORCE_VAL, assert SW_MODE and STOP_EN, preserve clock/polarity.
    unsafe {
        let config = (bl706_hal::pac::PWM.as_ptr() as *mut u8)
            .add(0x50)
            .cast::<u32>();
        let old = config.read_volatile();
        config.write_volatile((old & !(1 << 4)) | (1 << 5) | (1 << 6));
        core::arch::asm!("fence iorw, iorw");
    }
}
