//! UI state independent of the display, GPIO and heater hardware.

#[cfg(test)]
use crate::motion::Orientation;
use crate::settings::Settings;
pub use crate::settings::{BRIGHTNESS, MIN_SETPOINT_C, RotationMode};

pub const MAX_SETPOINT_C: u16 = crate::heater::MAX_TARGET_C;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Plus,
    Minus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonEvent {
    Short(Button),
    Long(Button),
    BothPressed,
    BothHeld,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Main,
    Brightness,
    Rotation,
    Measurements,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ui {
    pub screen: Screen,
    pub settings: Settings,
    pub heating_requested: bool,
    chord_started_heating: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            screen: Screen::Main,
            settings: Settings::default(),
            heating_requested: false,
            chord_started_heating: false,
        }
    }
}

impl Ui {
    pub fn with_settings(settings: Settings) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }
    /// A long press is its own event and never also applies a short press.
    pub fn handle(&mut self, event: ButtonEvent) -> bool {
        let previous = *self;
        match event {
            ButtonEvent::BothPressed => {
                self.chord_started_heating = self.heating_requested;
                // Stopping requires only a debounced press, not the hold time.
                self.heating_requested = false;
            }
            ButtonEvent::BothHeld => {
                if !self.chord_started_heating {
                    self.heating_requested = true;
                }
            }
            ButtonEvent::Long(Button::Plus) => {
                self.screen = match self.screen {
                    Screen::Main | Screen::Measurements => Screen::Brightness,
                    Screen::Brightness => Screen::Rotation,
                    Screen::Rotation => Screen::Main,
                };
            }
            ButtonEvent::Long(Button::Minus) => {
                self.screen = match self.screen {
                    Screen::Measurements => Screen::Main,
                    _ => Screen::Measurements,
                };
            }
            ButtonEvent::Short(button) => match self.screen {
                Screen::Main => {
                    self.settings.setpoint_c = match button {
                        Button::Plus => (self.settings.setpoint_c + 5).min(MAX_SETPOINT_C),
                        Button::Minus => self
                            .settings
                            .setpoint_c
                            .saturating_sub(5)
                            .max(MIN_SETPOINT_C),
                    };
                }
                Screen::Brightness => {
                    self.settings.brightness = match button {
                        Button::Plus => (self.settings.brightness + 1).min(BRIGHTNESS.len() as u8),
                        Button::Minus => self.settings.brightness.saturating_sub(1).max(1),
                    };
                }
                Screen::Measurements => {}
                Screen::Rotation => {
                    self.settings.rotation = match (self.settings.rotation, button) {
                        (RotationMode::Auto, Button::Plus)
                        | (RotationMode::Left, Button::Minus) => RotationMode::Right,
                        (RotationMode::Right, Button::Plus)
                        | (RotationMode::Auto, Button::Minus) => RotationMode::Left,
                        _ => RotationMode::Auto,
                    };
                }
            },
        }
        *self != previous
    }

    pub fn contrast(&self) -> u8 {
        self.settings.contrast()
    }

    pub fn brightness_level(&self) -> usize {
        usize::from(self.settings.brightness)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setpoint_steps_are_bounded_in_both_directions() {
        let mut ui = Ui::default();
        assert!(ui.handle(ButtonEvent::Short(Button::Plus)));
        assert_eq!(ui.settings.setpoint_c, 155);
        assert!(ui.handle(ButtonEvent::Short(Button::Minus)));
        assert_eq!(ui.settings.setpoint_c, 150);
        for _ in 0..1000 {
            ui.handle(ButtonEvent::Short(Button::Minus));
        }
        assert_eq!(ui.settings.setpoint_c, MIN_SETPOINT_C);
        assert!(!ui.handle(ButtonEvent::Short(Button::Minus)));
        for _ in 0..1000 {
            ui.handle(ButtonEvent::Short(Button::Plus));
        }
        assert_eq!(ui.settings.setpoint_c, MAX_SETPOINT_C);
        assert!(!ui.handle(ButtonEvent::Short(Button::Plus)));
    }

    #[test]
    fn long_press_switches_pages_without_changing_values() {
        let mut ui = Ui::default();
        ui.handle(ButtonEvent::Long(Button::Plus));
        assert_eq!(ui.screen, Screen::Brightness);
        assert_eq!(ui.settings.setpoint_c, 150);
        assert_eq!(ui.contrast(), 26);
        ui.handle(ButtonEvent::Short(Button::Plus));
        assert_eq!(ui.brightness_level(), 3);
        assert_eq!(ui.settings.setpoint_c, 150);
        ui.handle(ButtonEvent::Long(Button::Plus));
        assert_eq!(ui.screen, Screen::Rotation);
        ui.handle(ButtonEvent::Long(Button::Plus));
        assert_eq!(ui.screen, Screen::Main);
        assert_eq!(ui.contrast(), 64);
    }

    #[test]
    fn brightness_clamps_and_never_blanks_the_panel() {
        let mut ui = Ui::default();
        ui.handle(ButtonEvent::Long(Button::Plus));
        for _ in 0..100 {
            ui.handle(ButtonEvent::Short(Button::Minus));
        }
        assert_eq!(ui.contrast(), 8);
        assert!(!ui.handle(ButtonEvent::Short(Button::Minus)));
        for _ in 0..100 {
            ui.handle(ButtonEvent::Short(Button::Plus));
        }
        assert_eq!(ui.contrast(), 101);
        assert!(!ui.handle(ButtonEvent::Short(Button::Plus)));
    }

    #[test]
    fn long_minus_toggles_measurements_without_changing_settings() {
        let mut ui = Ui::default();
        assert!(ui.handle(ButtonEvent::Long(Button::Minus)));
        assert_eq!(ui.screen, Screen::Measurements);
        assert!(!ui.handle(ButtonEvent::Short(Button::Plus)));
        assert!(!ui.handle(ButtonEvent::Short(Button::Minus)));
        assert!(ui.handle(ButtonEvent::Long(Button::Minus)));
        assert_eq!(ui.screen, Screen::Main);
        ui.handle(ButtonEvent::Long(Button::Plus));
        assert!(ui.handle(ButtonEvent::Long(Button::Minus)));
        assert_eq!(ui.screen, Screen::Measurements);
        assert_eq!(ui.settings.setpoint_c, 150);
        assert_eq!(ui.contrast(), 26);
    }
    #[test]
    fn heating_starts_only_on_deliberate_chord_and_stops_on_press() {
        let mut ui = Ui::default();
        ui.handle(ButtonEvent::BothPressed);
        assert!(!ui.heating_requested);
        ui.handle(ButtonEvent::BothHeld);
        assert!(ui.heating_requested);
        ui.handle(ButtonEvent::BothPressed);
        assert!(!ui.heating_requested);
        ui.handle(ButtonEvent::BothHeld);
        assert!(!ui.heating_requested);
    }

    #[test]
    fn rotation_setting_cycles_and_manual_mode_overrides_the_sensor() {
        let mut ui = Ui::default();
        ui.handle(ButtonEvent::Long(Button::Plus));
        ui.handle(ButtonEvent::Long(Button::Plus));
        assert_eq!(ui.screen, Screen::Rotation);
        assert_eq!(
            ui.settings.rotation.orientation(Orientation::Left),
            Orientation::Left
        );
        ui.handle(ButtonEvent::Short(Button::Plus));
        assert_eq!(ui.settings.rotation, RotationMode::Right);
        assert_eq!(
            ui.settings.rotation.orientation(Orientation::Left),
            Orientation::Right
        );
        ui.handle(ButtonEvent::Short(Button::Plus));
        assert_eq!(ui.settings.rotation, RotationMode::Left);
        assert_eq!(
            ui.settings.rotation.orientation(Orientation::Right),
            Orientation::Left
        );
        ui.handle(ButtonEvent::Short(Button::Plus));
        assert_eq!(ui.settings.rotation, RotationMode::Auto);
        ui.handle(ButtonEvent::Short(Button::Minus));
        assert_eq!(ui.settings.rotation, RotationMode::Left);
        assert_eq!(ui.settings.setpoint_c, MIN_SETPOINT_C);
        assert!(!ui.heating_requested);
    }
}
