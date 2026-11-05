//! Explicit RAM commands for bounded hardware qualification; absent normally.
use bl706_hal::portable_atomic::{AtomicU32, Ordering};
use embassy_time::Instant;
use pinecil_rs::heater::Request;

/// JTAG writes only this RAM word, never peripheral MMIO. Commands:
/// 1 = 5 W / 80 C / 8 s, 2 = 10 W / 150 C / 30 s, 3 = stop,
/// 4 = inject sensor failure, 5 = watchdog test (output stopped first),
/// 6 = withhold task polling for 350 ms to test stale-command cutoff.
#[used]
#[unsafe(no_mangle)]
pub static HEATER_TEST_COMMAND: Mailbox = Mailbox {
    magic: 0x4854_5231,
    command: AtomicU32::new(0),
};
#[repr(C)]
pub struct Mailbox {
    magic: u32,
    command: AtomicU32,
}

pub struct Test {
    until_ms: u64,
    request: Request,
    pub inject_sensor_fault: bool,
    pub test_watchdog: bool,
    pub stall_task: bool,
    /// Keep test mode explicitly OFF after a bench run, ignoring any UI hold.
    active: bool,
}
impl Default for Test {
    fn default() -> Self {
        Self {
            until_ms: 0,
            request: Request::OFF,
            inject_sensor_fault: false,
            test_watchdog: false,
            stall_task: false,
            active: false,
        }
    }
}
impl Test {
    pub fn update(&mut self, normal: Request) -> Request {
        let now = Instant::now().as_millis();
        let command = HEATER_TEST_COMMAND.command.swap(0, Ordering::Relaxed);
        if command != 0 {
            log::info!("Heater hardware test command={}", command);
            self.active = true;
        }
        match command {
            1 | 2 => {
                let (limit_mw, target_c, duration) = if command == 1 {
                    (5000, 80, 8000)
                } else {
                    (10_000, 150, 30_000)
                };
                self.request = Request {
                    enabled: true,
                    target_c,
                    limit_mw,
                };
                self.until_ms = now + duration;
            }
            3 => self.request = Request::OFF,
            4 => self.inject_sensor_fault = true,
            5 => self.test_watchdog = true,
            6 => self.stall_task = true,
            _ => {}
        }
        if now >= self.until_ms {
            self.request = Request::OFF;
        }
        if self.active { self.request } else { normal }
    }
}
