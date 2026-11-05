use crate::SharedI2c;
#[cfg(feature = "pd-diagnostics")]
use crate::pd_trace;
use bl706_hal::gpio;
use core::fmt::Write;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
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
use pinecil_rs::app::{AppState, StateReceiver};
use pinecil_rs::power::PowerStatus;
use pinecil_rs::ui::{Screen, Ui};
use ssd1306::{I2CDisplayInterface, Ssd1306Async, prelude::*};

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
pub async fn run(
    i2c: SharedI2c,
    reset: gpio::Pin<3>,
    mut snapshots: StateReceiver<'static, CriticalSectionRawMutex>,
) {
    let mut reset = reset.into_output(false);
    let mut oled = Ssd1306Async::new(
        I2CDisplayInterface::new(DisplayI2c(i2c)),
        DisplaySize96x16,
        DisplayRotation::Rotate0,
    )
    .into_buffered_graphics_mode();
    let mut alive = true;
    let mut next_refresh = Instant::now() + Duration::from_secs(1);
    let mut needs_init = true;
    let mut applied_contrast = None;
    loop {
        if needs_init {
            oled.reset(&mut reset, &mut embassy_time::Delay)
                .await
                .unwrap();
            Timer::after_millis(100).await;
            if let Err(error) = oled.init().await {
                log::warn!("OLED init failed: {:?}; retrying", error);
                Timer::after_millis(250).await;
                continue;
            }
            needs_init = false;
            applied_contrast = None;
            log::info!("OLED ready; heater OFF");
        }
        // Always use the latest snapshot after initialization or recovery.
        // State changes during a flush are coalesced, never queued as frames.
        let AppState { ui, power } = snapshots.get().await;
        if applied_contrast != Some(ui.contrast()) {
            if let Err(error) = oled
                .set_brightness(Brightness::custom(0xf, ui.contrast()))
                .await
            {
                log::warn!("OLED brightness failed: {:?}; resetting display", error);
                needs_init = true;
                Timer::after_millis(250).await;
                continue;
            }
            applied_contrast = Some(ui.contrast());
        }
        #[cfg(feature = "pd-diagnostics")]
        let render_start = Instant::now();
        render(&mut oled, &ui, alive, power).await.unwrap();
        #[cfg(feature = "pd-diagnostics")]
        pd_trace::profile(4, render_start);
        if let Err(error) = oled.flush().await {
            log::warn!("OLED flush failed: {:?}; resetting display", error);
            needs_init = true;
            Timer::after_millis(250).await;
            continue;
        }
        match select(snapshots.changed(), Timer::at(next_refresh)).await {
            Either::First(_) => {}
            Either::Second(()) => {
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
