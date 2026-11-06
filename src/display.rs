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
use pinecil_rs::heater::State as HeaterState;
use pinecil_rs::motion::Orientation;
use pinecil_rs::power::PowerStatus;
use pinecil_rs::sensors::{Reading, SensorFault};
use pinecil_rs::ui::Screen;
use ssd1306::{I2CDisplayInterface, Ssd1306Async, prelude::*};

async fn render<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    state: &AppState,
    alive: bool,
) -> Result<(), D::Error> {
    let AppState {
        ui,
        power,
        measurements,
        ..
    } = state;
    #[cfg(feature = "pd-diagnostics")]
    let prepare_start = Instant::now();
    display.clear(BinaryColor::Off)?;
    let mut top: String<24> = String::new();
    let mut power_text: String<32> = String::new();
    match *power {
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
            match measurements.heater.state {
                HeaterState::Fault(fault) => write!(top, "ERR {}", fault.label()).unwrap(),
                HeaterState::Heating | HeaterState::Regulating => {
                    if let Reading::Value(tip) = measurements.tip_centi_c {
                        write!(top, "{}/{}C", tip / 100, measurements.heater.target_c).unwrap();
                    }
                }
                HeaterState::Off => write!(top, "SET {}C", ui.settings.setpoint_c).unwrap(),
            }
            power_text.as_str()
        }
        Screen::Brightness => {
            write!(top, "LIGHT {}/4", ui.brightness_level()).unwrap();
            "+/- HOLD+:ROT"
        }
        Screen::Rotation => {
            write!(top, "ROT {}", ui.settings.rotation.label()).unwrap();
            "+/- HOLD+:BACK"
        }
        Screen::Measurements => {
            match measurements.tip_centi_c {
                Reading::Value(temperature) => write!(top, "TIP {}C", temperature / 100).unwrap(),
                Reading::Fault(SensorFault::Saturated) => top.push_str("TIP SAT").unwrap(),
                Reading::Fault(_) => top.push_str("TIP ERR").unwrap(),
                Reading::Pending => top.push_str("TIP ---").unwrap(),
            }
            power_text.clear();
            match measurements.input_mv {
                Reading::Value(mv) => {
                    write!(power_text, "IN {}.{}V", mv / 1000, mv % 1000 / 100).unwrap()
                }
                _ => power_text.push_str("IN --.-V").unwrap(),
            }
            match measurements.handle_centi_c {
                Reading::Value(temperature) => {
                    write!(power_text, " H{}C", temperature / 100).unwrap()
                }
                _ => power_text.push_str(" H--C").unwrap(),
            }
            power_text.as_str()
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
        let status = match measurements.heater.state {
            HeaterState::Off => "OFF",
            HeaterState::Heating => "ON",
            HeaterState::Regulating => "REG",
            HeaterState::Fault(_) => "ERR",
        };
        Text::with_baseline(status, Point::new(78, 0), large, Baseline::Top).draw(&mut header)?;
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
    let mut applied_orientation = None;
    let mut last_frame: Option<(AppState, bool)> = None;
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
            applied_orientation = None;
            last_frame = None;
            log::info!("OLED ready");
        }
        // Always use the latest snapshot after initialization or recovery.
        // State changes during a flush are coalesced, never queued as frames.
        let state = snapshots.get().await;
        let ui = state.ui;
        let orientation = state.display_orientation();
        if applied_orientation != Some(orientation) {
            let rotation = match orientation {
                Orientation::Right => DisplayRotation::Rotate0,
                Orientation::Left => DisplayRotation::Rotate180,
            };
            if let Err(error) = oled.set_rotation(rotation).await {
                log::warn!("OLED rotation failed: {:?}; resetting display", error);
                needs_init = true;
                Timer::after_millis(250).await;
                continue;
            }
            applied_orientation = Some(orientation);
            // Both controller scan direction and buffered addressing change.
            last_frame = None;
        }
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
        // Raw ADC changes do not redraw the main/brightness pages. Avoid
        // adding OLED bus traffic just because measurements arrive at 4 Hz.
        let redraw = last_frame.is_none_or(|(previous, previous_alive)| {
            previous.ui != ui
                || previous.power != state.power
                || previous_alive != alive
                || previous.measurements.heater.state != state.measurements.heater.state
                || ui.screen == Screen::Main
                    && matches!(
                        state.measurements.heater.state,
                        HeaterState::Heating | HeaterState::Regulating
                    )
                    && previous.measurements.tip_centi_c != state.measurements.tip_centi_c
                || ui.screen == Screen::Measurements && previous.measurements != state.measurements
        });
        if redraw {
            #[cfg(feature = "pd-diagnostics")]
            let render_start = Instant::now();
            render(&mut oled, &state, alive).await.unwrap();
            #[cfg(feature = "pd-diagnostics")]
            pd_trace::profile(4, render_start);
            if let Err(error) = oled.flush().await {
                log::warn!("OLED flush failed: {:?}; resetting display", error);
                needs_init = true;
                Timer::after_millis(250).await;
                continue;
            }
            last_frame = Some((state, alive));
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
