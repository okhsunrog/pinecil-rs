#![no_std]
#![no_main]

mod board;
mod buttons;
mod pd;
#[cfg(feature = "pd-diagnostics")]
mod pd_trace;

use bl706_hal::{gpio, i2c::I2c};
use board::{Logger, banner, board};
use core::fmt::Write;
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_futures::select::{Either3, select3};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::{Duration, Instant, Timer};
use embedded_graphics::{
    mono_font::{
        MonoTextStyle,
        ascii::{FONT_5X8, FONT_6X10},
    },
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::{Baseline, Text},
};
use heapless::String;
use pinecil_rs::power::PowerStatus;
use pinecil_rs::ui::{Screen, Ui};
use ssd1306::{I2CDisplayInterface, Ssd1306Async, prelude::*};
use static_cell::StaticCell;

static EXECUTOR: StaticCell<bl706_hal::Executor> = StaticCell::new();
static PD_EXECUTOR: bl706_hal::InterruptExecutor = bl706_hal::InterruptExecutor::new();
static I2C_BUS: StaticCell<Mutex<CriticalSectionRawMutex, I2c>> = StaticCell::new();
pub type SharedI2c = I2cDevice<'static, CriticalSectionRawMutex, I2c>;

#[embassy_executor::task]
async fn heartbeat(mut log: Logger, mut heater_off: gpio::Output<21>) {
    banner(&mut log, "UI: heater OFF");
    let mut tick = 0u32;
    loop {
        heater_off.set_low();
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
        #[cfg(feature = "pd-diagnostics")]
        pd_trace::profile(1, start);
        tick = tick.wrapping_add(1);
        Timer::after_secs(1).await;
    }
}

async fn render<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    ui: &Ui,
    alive: bool,
    power: PowerStatus,
) -> Result<(), D::Error> {
    #[cfg(feature = "pd-diagnostics")]
    let prepare_start = Instant::now();
    display.clear(BinaryColor::Off)?;
    let mut top: String<24> = String::new();
    let mut power_text: String<32> = String::new();
    match power {
        PowerStatus::Waiting => power_text.push_str("PD WAIT HOLD+:MENU").unwrap(),
        PowerStatus::Requesting { mv } => write!(power_text, "PD {}V REQUEST", mv / 1000).unwrap(),
        PowerStatus::Contract { mv, ma } => write!(
            power_text,
            "PD {}V {}.{}A",
            mv / 1000,
            ma / 1000,
            (ma % 1000) / 100
        )
        .unwrap(),
        PowerStatus::Error => power_text.push_str("PD ERROR").unwrap(),
    }
    let footer = match ui.screen {
        Screen::Main => {
            write!(top, "SET {}C", ui.setpoint_c).unwrap();
            power_text.as_str()
        }
        Screen::Brightness => {
            write!(top, "LIGHT {}/4", ui.brightness_level()).unwrap();
            "+/- HOLD+:BACK"
        }
    };
    #[cfg(feature = "pd-diagnostics")]
    pd_trace::profile(6, prepare_start);
    // Drawing is CPU work. Bound each poll of this task so the PD response
    // timer can be serviced even while the framebuffer is being prepared.
    embassy_futures::yield_now().await;
    #[cfg(feature = "pd-diagnostics")]
    let header_start = Instant::now();
    {
        // These uppercase 6x10 glyphs fit in the first eight pixel rows.
        // Clip their blank font padding to give the footer a full eight rows.
        let mut header = display.clipped(&Rectangle::new(Point::zero(), Size::new(96, 8)));
        let large = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
        Text::with_baseline(&top, Point::zero(), large, Baseline::Top).draw(&mut header)?;
        Text::with_baseline("OFF", Point::new(78, 0), large, Baseline::Top).draw(&mut header)?;
    }
    #[cfg(feature = "pd-diagnostics")]
    pd_trace::profile(7, header_start);
    embassy_futures::yield_now().await;
    #[cfg(feature = "pd-diagnostics")]
    let footer_start = Instant::now();
    Text::with_baseline(
        footer,
        Point::new(0, 8),
        MonoTextStyle::new(&FONT_5X8, BinaryColor::On),
        Baseline::Top,
    )
    .draw(display)?;
    if alive {
        Rectangle::new(Point::new(93, 13), Size::new(3, 3))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)?;
    }
    #[cfg(feature = "pd-diagnostics")]
    pd_trace::profile(8, footer_start);
    embassy_futures::yield_now().await;
    Ok(())
}

#[embassy_executor::task]
async fn display(i2c: SharedI2c, reset: gpio::Pin<3>) {
    let mut reset = reset.into_output(false);
    let mut oled = Ssd1306Async::new(
        I2CDisplayInterface::new(DisplayI2c(i2c)),
        DisplaySize96x16,
        DisplayRotation::Rotate0,
    )
    .into_buffered_graphics_mode();
    oled.reset(&mut reset, &mut embassy_time::Delay)
        .await
        .unwrap();
    Timer::after_millis(100).await;
    let mut ui = Ui::default();
    let initialized = async {
        oled.init().await?;
        oled.set_brightness(Brightness::custom(0xf, ui.contrast()))
            .await
    }
    .await;
    if let Err(error) = initialized {
        rtt_target::rprintln!("OLED init failed: {:?}; heartbeat continues", error);
        return;
    }
    log::info!("UI ready; heater OFF");
    let mut alive = true;
    let mut power = PowerStatus::Waiting;
    let mut next_refresh = Instant::now() + Duration::from_secs(1);
    loop {
        #[cfg(feature = "pd-diagnostics")]
        let render_start = Instant::now();
        render(&mut oled, &ui, alive, power).await.unwrap();
        #[cfg(feature = "pd-diagnostics")]
        pd_trace::profile(4, render_start);
        if let Err(error) = oled.flush().await {
            rtt_target::rprintln!("OLED flush failed: {:?}; resetting display", error);
            Timer::after_millis(250).await;
            let _ = oled.reset(&mut reset, &mut embassy_time::Delay).await;
            if let Err(error) = oled.init().await {
                rtt_target::rprintln!("OLED recovery init failed: {:?}", error);
            } else {
                let _ = oled
                    .set_brightness(Brightness::custom(0xf, ui.contrast()))
                    .await;
            }
            continue;
        }
        match select3(
            buttons::EVENTS.receive(),
            pd::STATUS.wait(),
            Timer::at(next_refresh),
        )
        .await
        {
            Either3::First(event) => {
                let contrast = ui.contrast();
                ui.handle(event);
                log::debug!(
                    "UI event={:?} screen={:?} setpoint={} heater=OFF contrast={}",
                    event,
                    ui.screen,
                    ui.setpoint_c,
                    ui.contrast()
                );
                if contrast != ui.contrast()
                    && let Err(error) = oled
                        .set_brightness(Brightness::custom(0xf, ui.contrast()))
                        .await
                {
                    rtt_target::rprintln!("OLED brightness failed: {:?}", error);
                    return;
                }
            }
            Either3::Second(status) => {
                power = status;
            }
            Either3::Third(()) => {
                alive = !alive;
                next_refresh = Instant::now() + Duration::from_secs(1);
            }
        }
    }
}

struct DisplayI2c(SharedI2c);
impl embedded_hal_async::i2c::ErrorType for DisplayI2c {
    type Error = <SharedI2c as embedded_hal_async::i2c::ErrorType>::Error;
}
impl embedded_hal_async::i2c::I2c for DisplayI2c {
    async fn transaction(
        &mut self,
        address: u8,
        operations: &mut [embedded_hal_async::i2c::Operation<'_>],
    ) -> Result<(), Self::Error> {
        let result = self.0.transaction(address, operations).await;
        if let Err(error) = &result {
            log::warn!("OLED I2C: {:?}", error);
        }
        // A framebuffer flush performs many consecutive I2C transactions.
        // Let the waiting PD task run after releasing the shared-bus mutex;
        // otherwise this task can reacquire it for the entire flush.
        embassy_futures::yield_now().await;
        result
    }
}

#[unsafe(no_mangle)]
extern "C" fn __bl706_main() -> ! {
    let b = board();
    let bus = I2C_BUS.init(Mutex::new(b.i2c));
    // PD preempts framebuffer rendering and thread-mode formatting. Async
    // I2C retains shared bus ownership; no blocking wait is used in the IRQ.
    PD_EXECUTOR
        .start(b.software_interrupt)
        .spawn(pd::run(I2cDevice::new(bus), b.pd_irq).unwrap());
    EXECUTOR.init(bl706_hal::Executor::new()).run(|s| {
        s.spawn(heartbeat(b.log, b.heater_off).unwrap());
        s.spawn(buttons::plus(b.key_plus).unwrap());
        s.spawn(buttons::minus(b.key_minus).unwrap());
        s.spawn(display(I2cDevice::new(bus), b.oled_reset).unwrap());
    })
}
