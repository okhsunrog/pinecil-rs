//! UI state independent of the display, GPIO and heater hardware.

pub const MIN_SETPOINT_C: u16 = 150;
pub const MAX_SETPOINT_C: u16 = 450;
pub const BRIGHTNESS: [u8; 4] = [8, 26, 64, 101];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Plus,
    Minus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonEvent {
    Short(Button),
    Long(Button),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Main,
    Brightness,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ui {
    pub screen: Screen,
    pub setpoint_c: u16,
    brightness_index: usize,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            screen: Screen::Main,
            setpoint_c: 300,
            brightness_index: 1,
        }
    }
}

impl Ui {
    /// A long press is its own event and never also applies a short press.
    pub fn handle(&mut self, event: ButtonEvent) -> bool {
        let previous = *self;
        match event {
            ButtonEvent::Long(Button::Plus) => {
                self.screen = match self.screen {
                    Screen::Main => Screen::Brightness,
                    Screen::Brightness => Screen::Main,
                };
            }
            ButtonEvent::Long(Button::Minus) => {}
            ButtonEvent::Short(button) => match self.screen {
                Screen::Main => {
                    self.setpoint_c = match button {
                        Button::Plus => (self.setpoint_c + 5).min(MAX_SETPOINT_C),
                        Button::Minus => self.setpoint_c.saturating_sub(5).max(MIN_SETPOINT_C),
                    };
                }
                Screen::Brightness => {
                    self.brightness_index = match button {
                        Button::Plus => (self.brightness_index + 1).min(BRIGHTNESS.len() - 1),
                        Button::Minus => self.brightness_index.saturating_sub(1),
                    };
                }
            },
        }
        *self != previous
    }

    pub fn contrast(&self) -> u8 {
        BRIGHTNESS[self.brightness_index]
    }

    pub fn brightness_level(&self) -> usize {
        self.brightness_index + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setpoint_steps_are_bounded_in_both_directions() {
        let mut ui = Ui::default();
        assert!(ui.handle(ButtonEvent::Short(Button::Plus)));
        assert_eq!(ui.setpoint_c, 305);
        assert!(ui.handle(ButtonEvent::Short(Button::Minus)));
        assert_eq!(ui.setpoint_c, 300);
        for _ in 0..1000 {
            ui.handle(ButtonEvent::Short(Button::Minus));
        }
        assert_eq!(ui.setpoint_c, MIN_SETPOINT_C);
        assert!(!ui.handle(ButtonEvent::Short(Button::Minus)));
        for _ in 0..1000 {
            ui.handle(ButtonEvent::Short(Button::Plus));
        }
        assert_eq!(ui.setpoint_c, MAX_SETPOINT_C);
        assert!(!ui.handle(ButtonEvent::Short(Button::Plus)));
    }

    #[test]
    fn long_press_switches_pages_without_changing_values() {
        let mut ui = Ui::default();
        ui.handle(ButtonEvent::Long(Button::Plus));
        assert_eq!(ui.screen, Screen::Brightness);
        assert_eq!(ui.setpoint_c, 300);
        assert_eq!(ui.contrast(), 26);
        ui.handle(ButtonEvent::Short(Button::Plus));
        assert_eq!(ui.brightness_level(), 3);
        assert_eq!(ui.setpoint_c, 300);
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
    fn ignored_long_minus_preserves_both_pages() {
        let mut ui = Ui::default();
        assert!(!ui.handle(ButtonEvent::Long(Button::Minus)));
        ui.handle(ButtonEvent::Long(Button::Plus));
        assert!(!ui.handle(ButtonEvent::Long(Button::Minus)));
    }
}
