//! Presentation shared by the OLED task and host pixel previews.
use crate::{
    app::AppState,
    heater::State as HeaterState,
    power::PowerStatus,
    sensors::{Reading, SensorFault},
    ui::{Edit, MenuItem, Screen},
};
use core::fmt::Write;
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

pub const SIZE: Size = Size::new(96, 16);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub header: String<32>,
    pub footer: String<32>,
    pub heater: Option<&'static str>,
}
impl Frame {
    pub fn new(state: &AppState) -> Self {
        let mut frame = Self {
            header: String::new(),
            footer: String::new(),
            heater: None,
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
                    PowerStatus::Waiting => f.push_str("PD WAIT HOLD+:MENU").unwrap(),
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
                write!(
                    h,
                    "{} {}/{}",
                    item.label(),
                    item.position() + 1,
                    item.group().len()
                )
                .unwrap();
                match item {
                    MenuItem::Temperature => {
                        write!(f, "{}C HOLD+:EDIT", ui.settings.setpoint_c).unwrap()
                    }
                    MenuItem::Brightness => {
                        write!(f, "{}/4 HOLD+:EDIT", ui.settings.brightness).unwrap()
                    }
                    MenuItem::Rotation => {
                        write!(f, "{} HOLD+:EDIT", ui.settings.rotation.label()).unwrap()
                    }
                    MenuItem::Back | MenuItem::DisplayBack => f.push_str("HOLD+:BACK").unwrap(),
                    _ => f.push_str("HOLD+:OPEN -:BACK").unwrap(),
                }
            }
            Screen::Edit(edit) => {
                match edit {
                    Edit::Temperature(v) => write!(h, "SET {}C +/-", v).unwrap(),
                    Edit::Brightness(v) => write!(h, "LIGHT {}/4 +/-", v).unwrap(),
                    Edit::Rotation(v) => write!(h, "ROT {} +/-", v.label()).unwrap(),
                }
                f.push_str("HOLD+:OK -:CANCEL").unwrap();
            }
            Screen::ConfirmDefaults => {
                h.push_str("RESET SETTINGS?").unwrap();
                f.push_str("HOLD+:YES -:CANCEL").unwrap();
            }
            Screen::Measurements { .. } => {
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
        if matches!(ui.screen, Screen::Main | Screen::Measurements { .. }) {
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
        .draw(&mut display.clipped(&Rectangle::new(Point::new(0, 8), Size::new(92, 8))))?;
        if alive {
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
    fn every_menu_and_editor_fits_the_physical_panel() {
        let mut state = state();
        let mut screens = std::vec![
            Screen::Main,
            Screen::ConfirmDefaults,
            Screen::Measurements { from_menu: false }
        ];
        for &item in MenuItem::ROOT.iter().chain(MenuItem::DISPLAY.iter()) {
            screens.push(Screen::Menu(item));
        }
        for v in [150, 350] {
            screens.push(Screen::Edit(Edit::Temperature(v)));
        }
        for v in [1, 4] {
            screens.push(Screen::Edit(Edit::Brightness(v)));
        }
        for v in [RotationMode::Auto, RotationMode::Right, RotationMode::Left] {
            screens.push(Screen::Edit(Edit::Rotation(v)));
        }
        for screen in screens {
            state.ui.screen = screen;
            let frame = Frame::new(&state);
            let header_width = if frame.heater.is_some() { 78 } else { 96 };
            assert!(
                frame.header.len() * 6 <= header_width,
                "{:?}: {}",
                screen,
                frame.header
            );
            assert!(
                frame.footer.len() * 5 <= 92,
                "{:?}: {}",
                screen,
                frame.footer
            );
        }
    }
    #[test]
    fn fault_and_invalid_measurements_do_not_display_a_stale_temperature() {
        let mut state = state();
        state.measurements.heater.state = HeaterState::Fault(crate::heater::Fault::Sensor);
        assert_eq!(Frame::new(&state).header, "ERR ADC");
        state.ui.screen = Screen::Measurements { from_menu: true };
        state.measurements.tip_centi_c = Reading::Fault(SensorFault::Saturated);
        let frame = Frame::new(&state);
        assert_eq!(frame.header, "TIP SAT");
        assert_eq!(frame.heater, Some("ERR"));
    }
}
