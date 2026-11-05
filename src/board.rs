use bl706_hal::{gpio, i2c::I2c};
use core::fmt::Write;

pub struct Logger;
struct RttLog;
static RTT_LOG: RttLog = RttLog;
impl log::Log for RttLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }
    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            // Format outside RTT's critical section so an I2C FIFO interrupt
            // can run while formatting a diagnostic packet dump.
            let mut text = heapless::String::<256>::new();
            let _ = writeln!(
                text,
                "{} us={} {}",
                record.level(),
                embassy_time::Instant::now().as_micros(),
                record.args()
            );
            rtt_target::rprint!("{}", text.as_str());
        }
    }
    fn flush(&self) {}
}

impl core::fmt::Write for Logger {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        rtt_target::rprint!("{}", text);
        Ok(())
    }
}

pub struct Board {
    pub software_interrupt: bl706_hal::executor::InterruptToken,
    pub log: Logger,
    pub heater_off: gpio::Output<21>,
    pub i2c: I2c,
    pub oled_reset: gpio::Pin<3>,
    pub key_plus: gpio::Input<25>,
    pub key_minus: gpio::Input<28>,
    pub pd_irq: gpio::Input<16>,
}

pub fn board() -> Board {
    #[cfg(not(feature = "pd-diagnostics"))]
    rtt_target::rtt_init_print!(rtt_target::ChannelMode::NoBlockSkip, 4096);
    #[cfg(feature = "pd-diagnostics")]
    {
        let channels = rtt_target::rtt_init! {
            up: {
                0: { size: 8192, mode: rtt_target::ChannelMode::NoBlockSkip, name: "Terminal" }
                1: { size: 8192, mode: rtt_target::ChannelMode::NoBlockSkip, name: "PD I2C" }
            }
        };
        rtt_target::set_print_channel(channels.up.0);
        crate::pd_trace::init_channel(channels.up.1);
    }
    // Single core, before enabling interrupts or spawning any tasks.
    unsafe {
        log::set_logger_racy(&RTT_LOG).unwrap();
        log::set_max_level_racy(log::LevelFilter::Info);
    }
    rtt_target::rprintln!("Pinecil Rust firmware: XIP + FPU + EPR; initializing HAL");
    let p = bl706_hal::init();
    let heater_off = p.pins.p21.into_output(false);
    // Pinecil's BL706 has embedded flash and no PSRAM. SDK System_Init routes
    // GPIO23..28 to SF/PSRAM pads; without this GPIO25/28 do not see the keys.
    bl706_hal::pac::GLB
        .GPIO_USE_PSRAM__IO()
        .modify(|r| r.0 |= 0x3f);
    let mut i2c = I2c::new(p.i2c, p.pins.p10, p.pins.p11, p.clocks);
    i2c.set_frequency(p.clocks, 300_000).unwrap();
    let board = Board {
        software_interrupt: p.software_interrupt,
        log: Logger,
        heater_off,
        i2c,
        oled_reset: p.pins.p3,
        key_plus: p.pins.p25.into_input(gpio::Pull::Down),
        key_minus: p.pins.p28.into_input(gpio::Pull::Down),
        pd_irq: p.pins.p16.into_input(gpio::Pull::Up),
    };
    bl706_hal::enable_interrupts();
    board
}

pub fn banner(log: &mut Logger, name: &str) {
    let misa: usize;
    unsafe { core::arch::asm!("csrr {}, misa", out(reg) misa) };
    let _ = writeln!(log, "Pinecil {name}: nominal RC32M, misa={misa:08x}");
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    unsafe { core::arch::asm!("csrci mstatus, 8") };
    rtt_target::rprintln!("panic: {}", info);
    loop {
        core::hint::spin_loop();
    }
}
