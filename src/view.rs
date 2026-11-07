//! Presentation shared by the OLED task and host pixel previews.
use crate::{
    app::AppState,
    heater::State as HeaterState,
    power::PowerStatus,
    sensors::{Reading, SensorFault},
    ui::{MenuItem, Screen},
};
use core::fmt::Write;
use embedded_graphics::{
    mono_font::{
        MonoTextStyle,
        ascii::{FONT_5X8, FONT_6X10, FONT_7X13},
    },
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::{Baseline, Text},
};
use heapless::String;

pub const SIZE: Size = Size::new(96, 16);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub header: String<32>,
    pub footer: String<32>,
    pub heater: Option<&'static str>,
    pub value: Option<String<8>>,
    pub position: Option<(usize, usize)>,
}
impl Frame {
    pub fn new(state: &AppState) -> Self {
        let mut frame = Self {
            header: String::new(),
            footer: String::new(),
            heater: None,
            value: None,
            position: None,
        };
        let ui = state.ui;
        let measurements = state.measurements;
        let h = &mut frame.header;
        let f = &mut frame.footer;
        match ui.screen {
            Screen::Main => {
                match measurements.heater.state {
                    HeaterState::Fault(fault) => write!(h, "ERR {}", fault.label()).unwrap(),
                    HeaterState::Heating | HeaterState::Regulating => {
                        match measurements.tip_centi_c {
                            Reading::Value(tip) => {
                                write!(h, "{}/{}C", tip / 100, measurements.heater.target_c)
                                    .unwrap()
                            }
                            _ => h.push_str("TIP ---").unwrap(),
                        }
                    }
                    HeaterState::Off => write!(h, "SET {}C", ui.settings.setpoint_c).unwrap(),
                }
                match state.power {
                    PowerStatus::Waiting => f.push_str("PD WAIT -:SETTINGS").unwrap(),
                    PowerStatus::Requesting { mv } => {
                        write!(f, "PD {}V REQUEST", mv / 1000).unwrap()
                    }
                    PowerStatus::Contract { mv, ma } => {
                        write!(f, "PD {}V {}.{}A", mv / 1000, ma / 1000, ma % 1000 / 100).unwrap()
                    }
                    PowerStatus::Error => f.push_str("PD ERROR").unwrap(),
                }
            }
            Screen::Menu(item) => {
                frame.position = Some((item.position(), item.group().len()));
                let mut value = String::new();
                match item {
                    MenuItem::Soldering => {
                        h.push_str("SOLDERING").unwrap();
                        f.push_str("TIP SETTINGS").unwrap();
                    }
                    MenuItem::Display => {
                        h.push_str("DISPLAY").unwrap();
                        f.push_str("SCREEN SETTINGS").unwrap();
                    }
                    MenuItem::Advanced => {
                        h.push_str("ADVANCED").unwrap();
                        f.push_str("DEVICE SETTINGS").unwrap();
                    }
                    MenuItem::Temperature => {
                        h.push_str("SETPOINT").unwrap();
                        f.push_str("TEMP C").unwrap();
                        write!(value, "{}", ui.settings.setpoint_c).unwrap();
                    }
                    MenuItem::Brightness => {
                        h.push_str("SCREEN").unwrap();
                        f.push_str("BRIGHTNESS").unwrap();
                        write!(value, "{}/4", ui.settings.brightness).unwrap();
                    }
                    MenuItem::Rotation => {
                        h.push_str("SCREEN").unwrap();
                        f.push_str("ROTATION").unwrap();
                        value.push_str(ui.settings.rotation.label()).unwrap();
                    }
                    MenuItem::Defaults => {
                        h.push_str("RESTORE").unwrap();
                        f.push_str("DEFAULTS").unwrap();
                        value.push_str("GO").unwrap();
                    }
                }
                if !value.is_empty() {
                    frame.value = Some(value);
                }
            }
            Screen::Temperature => {
                write!(h, "SET {}C", ui.settings.setpoint_c).unwrap();
                f.push_str("+/- TEMP BOTH:BACK").unwrap();
            }
            Screen::ConfirmDefaults => {
                h.push_str("RESET SETTINGS?").unwrap();
                f.push_str("+ YES    - NO").unwrap();
            }
            Screen::Measurements => {
                match measurements.tip_centi_c {
                    Reading::Value(v) => write!(h, "TIP {}C", v / 100).unwrap(),
                    Reading::Fault(SensorFault::Saturated) => h.push_str("TIP SAT").unwrap(),
                    Reading::Fault(_) => h.push_str("TIP ERR").unwrap(),
                    Reading::Pending => h.push_str("TIP ---").unwrap(),
                }
                match measurements.input_mv {
                    Reading::Value(mv) => {
                        write!(f, "IN {}.{}V", mv / 1000, mv % 1000 / 100).unwrap()
                    }
                    _ => f.push_str("IN --.-V").unwrap(),
                }
                match measurements.handle_centi_c {
                    Reading::Value(v) => write!(f, " H{}C", v / 100).unwrap(),
                    _ => f.push_str(" H--C").unwrap(),
                }
            }
        }
        // Menu headings use the entire row. The status remains visible on the
        // main/measurement screens; a chord still stops heat on every screen.
        if matches!(
            ui.screen,
            Screen::Main | Screen::Temperature | Screen::Measurements
        ) {
            frame.heater = Some(match measurements.heater.state {
                HeaterState::Off => "OFF",
                HeaterState::Heating => "ON",
                HeaterState::Regulating => "REG",
                HeaterState::Fault(_) => "ERR",
            });
        }
        frame
    }

    pub fn draw_header<D: DrawTarget<Color = BinaryColor>>(
        &self,
        display: &mut D,
    ) -> Result<(), D::Error> {
        if let Some(value) = &self.value {
            let mut label = display.clipped(&Rectangle::new(Point::zero(), Size::new(54, 8)));
            Text::with_baseline(
                &self.header,
                Point::zero(),
                MonoTextStyle::new(&FONT_5X8, BinaryColor::On),
                Baseline::Top,
            )
            .draw(&mut label)?;
            let x = 92 - value.len() as i32 * 7;
            Text::with_baseline(
                value,
                Point::new(x, 1),
                MonoTextStyle::new(&FONT_7X13, BinaryColor::On),
                Baseline::Top,
            )
            .draw(display)?;
            return Ok(());
        }
        let large = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
        // Uppercase glyphs occupy eight rows; clip the font's blank padding.
        let width = if self.heater.is_some() {
            78
        } else {
            SIZE.width
        };
        Text::with_baseline(&self.header, Point::zero(), large, Baseline::Top)
            .draw(&mut display.clipped(&Rectangle::new(Point::zero(), Size::new(width, 8))))?;
        if let Some(status) = self.heater {
            Text::with_baseline(status, Point::new(78, 0), large, Baseline::Top)
                .draw(&mut display.clipped(&Rectangle::new(Point::zero(), Size::new(96, 8))))?;
        }
        Ok(())
    }
    pub fn draw_footer<D: DrawTarget<Color = BinaryColor>>(
        &self,
        display: &mut D,
        alive: bool,
    ) -> Result<(), D::Error> {
        Text::with_baseline(
            &self.footer,
            Point::new(0, 8),
            MonoTextStyle::new(&FONT_5X8, BinaryColor::On),
            Baseline::Top,
        )
        .draw(&mut display.clipped(&Rectangle::new(
            Point::new(0, 8),
            Size::new(if self.value.is_some() { 54 } else { 92 }, 8),
        )))?;
        if let Some((position, count)) = self.position {
            Rectangle::new(
                Point::new(95, (16 * position / count) as i32),
                Size::new(1, (16 / count) as u32),
            )
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(display)?;
        } else if alive {
            Rectangle::new(Point::new(93, 13), Size::new(3, 3))
                .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                .draw(display)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{motion::MotionState, sensors::Measurements, settings::RotationMode, ui::Ui};
    fn state() -> AppState {
        AppState {
            ui: Ui::default(),
            power: PowerStatus::Contract {
                mv: 28_000,
                ma: 3000,
            },
            measurements: Measurements::WAITING,
            motion: MotionState::WAITING,
        }
    }
    #[test]
    fn every_setting_fits_the_label_value_layout() {
        let mut state = state();
        let items = MenuItem::ROOT
            .iter()
            .chain(MenuItem::SOLDERING.iter())
            .chain(MenuItem::DISPLAY.iter())
            .chain(MenuItem::ADVANCED.iter());
        for &item in items {
            state.ui.screen = Screen::Menu(item);
            for rotation in [RotationMode::Auto, RotationMode::Right, RotationMode::Left] {
                state.ui.settings.rotation = rotation;
                let frame = Frame::new(&state);
                if let Some(value) = &frame.value {
                    assert!(frame.header.len() * 5 <= 54);
                    assert!(frame.footer.len() * 5 <= 54);
                    assert!(value.len() * 7 <= 38);
                } else {
                    assert!(frame.header.len() * 6 <= 94);
                    assert!(frame.footer.len() * 5 <= 94);
                }
            }
        }
        for screen in [
            Screen::Main,
            Screen::Temperature,
            Screen::Measurements,
            Screen::ConfirmDefaults,
        ] {
            state.ui.screen = screen;
            let frame = Frame::new(&state);
            assert!(frame.header.len() * 6 <= if frame.heater.is_some() { 78 } else { 96 });
            assert!(frame.footer.len() * 5 <= 92);
        }
    }
    #[test]
    fn fault_and_invalid_measurements_do_not_display_a_stale_temperature() {
        let mut state = state();
        state.measurements.heater.state = HeaterState::Fault(crate::heater::Fault::Sensor);
        assert_eq!(Frame::new(&state).header, "ERR ADC");
        state.ui.screen = Screen::Measurements;
        state.measurements.tip_centi_c = Reading::Fault(SensorFault::Saturated);
        let frame = Frame::new(&state);
        assert_eq!(frame.header, "TIP SAT");
        assert_eq!(frame.heater, Some("ERR"));
    }
}
