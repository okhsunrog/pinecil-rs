use crate::board::{Logger, banner};
#[cfg(feature = "pd-diagnostics")]
use crate::pd_trace;
use core::fmt::Write;
#[cfg(feature = "pd-diagnostics")]
use embassy_time::Instant;
use embassy_time::Timer;

#[embassy_executor::task]
pub async fn run(mut log: Logger) {
    banner(&mut log, "UI: heater OFF");
    let mut tick = 0u32;
    loop {
        #[cfg(feature = "pd-diagnostics")]
        let start = Instant::now();
        #[cfg(feature = "pd-diagnostics")]
        if pd_trace::PD_VERBOSE_HEARTBEAT.load(bl706_hal::portable_atomic::Ordering::Relaxed) != 0 {
            let gpio_start = Instant::now();
            let pads = bl706_hal::pac::GLB.GPIO_CFGCTL30().read().0;
            let gpio_irq = bl706_hal::pac::GLB.GPIO_INT_STAT1().read().0;
            pd_trace::profile(2, gpio_start);
            let format_start = Instant::now();
            let _ = writeln!(
                log,
                "heartbeat tick={tick} us={} pads={:08x} gpio_irq={:08x}",
                start.as_micros(),
                pads,
                gpio_irq,
            );
            pd_trace::profile(3, format_start);
        } else {
            let _ = writeln!(log, "heartbeat tick={tick}");
        }
        #[cfg(not(feature = "pd-diagnostics"))]
        let _ = writeln!(log, "heartbeat tick={tick}");
        #[cfg(feature = "ble")]
        if tick.is_multiple_of(5) {
            let m = bl706_ble::take_metrics();
            log::info!(
                "BLE irq={} irq_max_us={} schedule_max_us={} wake_max_us={}",
                m.interrupts,
                m.max_interrupt_us,
                m.max_schedule_us,
                m.max_wake_to_worker_us
            );
        }
        #[cfg(feature = "pd-diagnostics")]
        pd_trace::profile(1, start);
        tick = tick.wrapping_add(1);
        Timer::after_secs(1).await;
    }
}
