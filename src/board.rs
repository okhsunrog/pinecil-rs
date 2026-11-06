use bl706_hal::adc::{Adc, AnalogPin, Config as AdcConfig, GainCalibration};
use bl706_hal::{gpio, i2c::I2c};
use core::fmt::Write;

pub struct Logger;
pub type BoardAdc = Adc<(AnalogPin<18>, AnalogPin<19>, AnalogPin<20>)>;
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
    #[cfg(feature = "ble")]
    pub ble: (bl706_ble::NativeTransport, bl706_ble::Runner),
    pub settings_store: Option<pinecil_rs::settings_store::Store<bl706_hal::flash::XipFlash>>,
    pub initial_settings: pinecil_rs::settings::Settings,
    pub software_interrupt: bl706_hal::executor::InterruptToken,
    pub log: Logger,
    pub commands: rtt_target::DownChannel,
    pub heater: crate::heater_io::Carrier,
    pub heater_timer: bl706_hal::timer::Timer<0>,
    pub watchdog: bl706_hal::watchdog::Watchdog,
    pub tip_probe: gpio::Output<24>,
    pub adc: BoardAdc,
    pub i2c: I2c,
    pub oled_reset: gpio::Pin<3>,
    pub key_plus: gpio::Input<25>,
    pub key_minus: gpio::Input<28>,
    pub pd_irq: gpio::Input<16>,
}

pub fn board() -> Board {
    #[cfg(not(feature = "pd-diagnostics"))]
    let commands = {
        let channels = rtt_target::rtt_init! {
            up: {
                0: { size: 4096, mode: rtt_target::ChannelMode::NoBlockSkip, name: "Terminal" }
            }
            down: {
                0: { size: 128, name: "Commands" }
            }
        };
        rtt_target::set_print_channel(channels.up.0);
        channels.down.0
    };
    #[cfg(feature = "pd-diagnostics")]
    let commands = {
        let channels = rtt_target::rtt_init! {
            up: {
                // Match the ordinary terminal size; leave room for application
                // task futures below BLE's reserved RF calibration scratch.
                0: { size: 4096, mode: rtt_target::ChannelMode::NoBlockSkip, name: "Terminal" }
                1: { size: 8192, mode: rtt_target::ChannelMode::NoBlockSkip, name: "PD I2C" }
            }
            down: {
                0: { size: 128, name: "Commands" }
            }
        };
        rtt_target::set_print_channel(channels.up.0);
        crate::pd_trace::init_channel(channels.up.1);
        channels.down.0
    };
    // Single core, before enabling interrupts or spawning any tasks.
    unsafe {
        log::set_logger_racy(&RTT_LOG).unwrap();
        log::set_max_level_racy(log::LevelFilter::Info);
    }
    rtt_target::rprintln!("Pinecil Rust firmware: XIP + FPU + EPR; initializing HAL");
    #[allow(unused_mut)]
    let mut p = bl706_hal::init_with_config(bl706_hal::clock::Config::pinecil_v2_144mhz())
        .expect("Pinecil clock initialization failed");
    log::info!(
        "Clocks: CPU {} MHz, bus {} MHz, flash {} MHz",
        p.clocks.cpu_hz() / 1_000_000,
        p.clocks.bus_hz() / 1_000_000,
        p.clocks.flash_hz() / 1_000_000
    );
    // The AC-coupled gate driver needs a carrier, approximately 5.33 kHz as
    // in IronOS. Pwm::new forces low before connecting the pad and stays off.
    let heater = bl706_hal::pwm::Pwm::new(
        p.pwm1,
        p.pins.p21,
        p.clocks,
        bl706_hal::pwm::Config {
            divider: (p.clocks.bus_hz() / 533_333) as u16,
            period: 100,
        },
    )
    .expect("heater PWM initialization failed");
    let (settings_store, initial_settings) = crate::storage::open(p.flash);
    #[cfg(feature = "ble")]
    let ble = {
        // RF calibration temporarily owns IRQ3 and analog hardware. Complete
        // it before the PD executor starts and before configuring ADC/I2C.
        bl706_hal::enable_interrupts();
        // SAFETY: the BLE linker reserves RF scratch/exchange RAM; this is the
        // sole radio/TRNG owner, XTAL32M is running, and IRQ3 is still unused.
        unsafe { bl706_ble::initialize(p.radio, &mut p.software_interrupt) }
            .expect("BLE controller initialization failed")
    };
    // Pinecil's BL706 has embedded flash and no PSRAM. SDK System_Init routes
    // GPIO23..28 to SF/PSRAM pads; without this GPIO25/28 do not see the keys.
    bl706_hal::pac::GLB
        .GPIO_USE_PSRAM__IO()
        .modify(|r| r.0 |= 0x3f);
    let gain_trim = GainCalibration::from_efuse_shadow();
    log::info!("ADC gain trim: {:?}", gain_trim);
    let adc = Adc::new(
        p.adc,
        (
            p.pins.p18.into_analog(),
            p.pins.p19.into_analog(),
            p.pins.p20.into_analog(),
        ),
        p.clocks,
        AdcConfig {
            calibration: gain_trim.ok(),
            ..Default::default()
        },
    );
    let mut i2c = I2c::new(p.i2c, p.pins.p10, p.pins.p11, p.clocks);
    i2c.set_frequency(p.clocks, 300_000).unwrap();
    let heater_timer = bl706_hal::timer::Timer::new(
        p.timer0,
        p.clocks,
        bl706_hal::timer::Config {
            clock: bl706_hal::timer::ClockSource::System,
            divider: (p.clocks.cpu_hz() / 1_000_000) as u16,
            mode: bl706_hal::timer::Mode::FreeRunning,
            compare: [u32::MAX; 3],
        },
    )
    .expect("heater cutoff timer initialization failed");
    let watchdog = bl706_hal::watchdog::Watchdog::new(
        p.watchdog,
        p.clocks,
        bl706_hal::watchdog::Config {
            timeout: core::time::Duration::from_millis(500),
            ..Default::default()
        },
    )
    .expect("heater watchdog initialization failed");
    log::info!("Reset reason: {:?}; heater starts OFF", p.reset_reason);
    let board = Board {
        settings_store,
        initial_settings,
        #[cfg(feature = "ble")]
        ble,
        software_interrupt: p.software_interrupt,
        log: Logger,
        commands,
        heater,
        heater_timer,
        watchdog,
        // Low reverse-biases the resistance probe diode during thermometry.
        tip_probe: p.pins.p24.into_output(false),
        adc,
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
    let _ = writeln!(log, "Pinecil {name}: misa={misa:08x}");
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    unsafe { core::arch::asm!("csrci mstatus, 8") };
    crate::heater_io::emergency_stop();
    rtt_target::rprintln!("panic: {}", info);
    loop {
        core::hint::spin_loop();
    }
}
