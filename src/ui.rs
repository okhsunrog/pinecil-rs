//! IronOS-style two-button settings navigation, independent of display I/O.
pub use crate::settings::{BRIGHTNESS, MIN_SETPOINT_C, RotationMode};
use crate::{
    motion::Orientation,
    settings::{Command, Settings},
};
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
    Repeat(Button),
    BothPressed,
    BothHeld,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Soldering,
    Display,
    Advanced,
    Temperature,
    Brightness,
    Rotation,
    Defaults,
}
impl MenuItem {
    pub const ROOT: [Self; 3] = [Self::Soldering, Self::Display, Self::Advanced];
    pub const SOLDERING: [Self; 1] = [Self::Temperature];
    pub const DISPLAY: [Self; 2] = [Self::Brightness, Self::Rotation];
    pub const ADVANCED: [Self; 1] = [Self::Defaults];
    pub fn group(self) -> &'static [Self] {
        match self {
            Self::Soldering | Self::Display | Self::Advanced => &Self::ROOT,
            Self::Temperature => &Self::SOLDERING,
            Self::Brightness | Self::Rotation => &Self::DISPLAY,
            Self::Defaults => &Self::ADVANCED,
        }
    }
    pub fn position(self) -> usize {
        self.group().iter().position(|item| *item == self).unwrap()
    }
    pub fn parent(self) -> Screen {
        match self {
            Self::Soldering | Self::Display | Self::Advanced => Screen::Main,
            Self::Temperature => Screen::Menu(Self::Soldering),
            Self::Brightness | Self::Rotation => Screen::Menu(Self::Display),
            Self::Defaults => Screen::Menu(Self::Advanced),
        }
    }
    fn next(self) -> Screen {
        self.group()
            .get(self.position() + 1)
            .copied()
            .map(Screen::Menu)
            .unwrap_or_else(|| self.parent())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Main,
    Menu(MenuItem),
    Temperature,
    Measurements,
    ConfirmDefaults,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ui {
    pub screen: Screen,
    pub settings: Settings,
    pub heating_requested: bool,
    chord_can_start: bool,
}
impl Default for Ui {
    fn default() -> Self {
        Self {
            screen: Screen::Main,
            settings: Settings::default(),
            heating_requested: false,
            chord_can_start: false,
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
    /// Menu changes apply immediately, but persistence waits until menu exit.
    pub fn settings_open(&self) -> bool {
        matches!(self.screen, Screen::Menu(_) | Screen::ConfirmDefaults)
    }
    pub fn handle(&mut self, event: ButtonEvent) -> bool {
        let before = *self;
        match event {
            ButtonEvent::BothPressed => {
                self.chord_can_start = self.screen == Screen::Main && !self.heating_requested;
                self.heating_requested = false;
                // A chord begun in a menu can only go back, even if holding it
                // takes us to the main screen before BothHeld arrives.
                self.screen = match self.screen {
                    Screen::Menu(item) => item.parent(),
                    Screen::ConfirmDefaults => Screen::Menu(MenuItem::Defaults),
                    _ => Screen::Main,
                };
            }
            ButtonEvent::BothHeld => {
                if self.chord_can_start && self.screen == Screen::Main {
                    self.heating_requested = true;
                }
                self.chord_can_start = false;
            }
            _ => {
                self.chord_can_start = false;
                match (self.screen, event) {
                    (Screen::Main, ButtonEvent::Short(Button::Minus))
                        if !self.heating_requested =>
                    {
                        self.screen = Screen::Menu(MenuItem::Soldering)
                    }
                    (Screen::Main, ButtonEvent::Short(_) | ButtonEvent::Long(Button::Plus)) => {
                        self.screen = Screen::Temperature
                    }
                    (Screen::Main, ButtonEvent::Long(Button::Minus)) => {
                        self.screen = Screen::Measurements
                    }
                    (
                        Screen::Temperature,
                        ButtonEvent::Short(button)
                        | ButtonEvent::Long(button)
                        | ButtonEvent::Repeat(button),
                    ) => {
                        let value = match button {
                            Button::Plus => (self.settings.setpoint_c + 5).min(MAX_SETPOINT_C),
                            Button::Minus => self
                                .settings
                                .setpoint_c
                                .saturating_sub(5)
                                .max(MIN_SETPOINT_C),
                        };
                        self.settings.apply(Command::Temperature(value));
                    }
                    (
                        Screen::Menu(item),
                        ButtonEvent::Short(Button::Minus)
                        | ButtonEvent::Long(Button::Minus)
                        | ButtonEvent::Repeat(Button::Minus),
                    ) => self.screen = item.next(),
                    (
                        Screen::Menu(item),
                        ButtonEvent::Short(Button::Plus)
                        | ButtonEvent::Long(Button::Plus)
                        | ButtonEvent::Repeat(Button::Plus),
                    ) => match item {
                        MenuItem::Soldering => self.screen = Screen::Menu(MenuItem::Temperature),
                        MenuItem::Display => self.screen = Screen::Menu(MenuItem::Brightness),
                        MenuItem::Advanced => self.screen = Screen::Menu(MenuItem::Defaults),
                        MenuItem::Temperature => {
                            self.settings.apply(Command::Temperature(
                                if self.settings.setpoint_c >= MAX_SETPOINT_C {
                                    MIN_SETPOINT_C
                                } else {
                                    self.settings.setpoint_c + 5
                                },
                            ));
                        }
                        MenuItem::Brightness => {
                            self.settings.apply(Command::Brightness(
                                self.settings.brightness % BRIGHTNESS.len() as u8 + 1,
                            ));
                        }
                        MenuItem::Rotation => {
                            self.settings
                                .apply(Command::Rotation(match self.settings.rotation {
                                    RotationMode::Auto => RotationMode::Right,
                                    RotationMode::Right => RotationMode::Left,
                                    RotationMode::Left => RotationMode::Auto,
                                }));
                        }
                        MenuItem::Defaults => {
                            if !matches!(event, ButtonEvent::Repeat(_)) {
                                self.screen = Screen::ConfirmDefaults;
                            }
                        }
                    },
                    (Screen::ConfirmDefaults, ButtonEvent::Short(Button::Plus)) => {
                        self.heating_requested = false;
                        self.settings.apply(Command::Defaults);
                        self.screen = Screen::Menu(MenuItem::Defaults);
                    }
                    (Screen::ConfirmDefaults, ButtonEvent::Short(Button::Minus)) => {
                        self.screen = Screen::Menu(MenuItem::Defaults)
                    }
                    (Screen::Measurements, ButtonEvent::Long(Button::Minus)) => {
                        self.screen = Screen::Main
                    }
                    _ => {}
                }
            }
        }
        *self != before
    }
    pub fn contrast(&self) -> u8 {
        self.settings.contrast()
    }
    pub fn display_orientation(&self, detected: Orientation) -> Orientation {
        self.settings.rotation.orientation(detected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const PLUS: ButtonEvent = ButtonEvent::Short(Button::Plus);
    const MINUS: ButtonEvent = ButtonEvent::Short(Button::Minus);
    const BACK: ButtonEvent = ButtonEvent::BothPressed;
    fn events(ui: &mut Ui, events: &[ButtonEvent]) {
        for &e in events {
            ui.handle(e);
        }
    }
    #[test]
    fn settings_use_short_presses_and_apply_values_without_an_editor() {
        let mut ui = Ui::default();
        events(&mut ui, &[MINUS, MINUS, PLUS]);
        assert_eq!(ui.screen, Screen::Menu(MenuItem::Brightness));
        assert!(ui.settings_open());
        events(&mut ui, &[PLUS, PLUS, PLUS]);
        assert_eq!(ui.settings.brightness, 1);
        assert_eq!(ui.contrast(), 8);
        events(&mut ui, &[MINUS, PLUS]);
        assert_eq!(ui.settings.rotation, RotationMode::Right);
        assert_eq!(
            ui.display_orientation(Orientation::Left),
            Orientation::Right
        );
        events(&mut ui, &[MINUS]);
        assert_eq!(ui.screen, Screen::Menu(MenuItem::Display));
        events(&mut ui, &[BACK, ButtonEvent::BothHeld]);
        assert_eq!(ui.screen, Screen::Main);
        assert!(!ui.settings_open());
        assert!(!ui.heating_requested);
    }
    #[test]
    fn passing_the_end_returns_to_the_parent_instead_of_wrapping() {
        let mut ui = Ui::default();
        events(&mut ui, &[MINUS, PLUS, MINUS]);
        assert_eq!(ui.screen, Screen::Menu(MenuItem::Soldering));
        events(&mut ui, &[MINUS, MINUS, MINUS]);
        assert_eq!(ui.screen, Screen::Main);
    }
    #[test]
    fn separate_temperature_adjustment_is_bounded_and_never_starts_heat() {
        let mut ui = Ui::default();
        ui.handle(PLUS);
        assert_eq!(ui.screen, Screen::Temperature);
        for _ in 0..100 {
            ui.handle(PLUS);
        }
        assert_eq!(ui.settings.setpoint_c, MAX_SETPOINT_C);
        assert!(!ui.handle(PLUS));
        for _ in 0..100 {
            ui.handle(MINUS);
        }
        assert_eq!(ui.settings.setpoint_c, MIN_SETPOINT_C);
        events(&mut ui, &[BACK, ButtonEvent::BothHeld]);
        assert_eq!(ui.screen, Screen::Main);
        assert!(!ui.heating_requested);
    }
    #[test]
    fn menu_temperature_wraps_through_the_available_values() {
        let mut ui = Ui::with_settings(Settings {
            setpoint_c: MAX_SETPOINT_C,
            ..Settings::default()
        });
        events(&mut ui, &[MINUS, PLUS, PLUS]);
        assert_eq!(ui.settings.setpoint_c, MIN_SETPOINT_C);
        assert!(!ui.heating_requested);
    }
    #[test]
    fn defaults_require_a_separate_short_confirmation() {
        let mut ui = Ui::with_settings(Settings {
            brightness: 4,
            ..Settings::default()
        });
        events(&mut ui, &[MINUS, MINUS, MINUS, PLUS, PLUS, MINUS]);
        assert_eq!(ui.settings.brightness, 4);
        events(&mut ui, &[PLUS, ButtonEvent::Repeat(Button::Plus)]);
        assert_eq!(ui.screen, Screen::ConfirmDefaults);
        assert_eq!(ui.settings.brightness, 4);
        ui.handle(PLUS);
        assert_eq!(ui.settings, Settings::default());
    }
    #[test]
    fn chords_can_stop_anywhere_but_only_start_when_begun_on_main() {
        let mut ui = Ui::default();
        assert!(!ui.handle(ButtonEvent::BothHeld));
        events(&mut ui, &[BACK, ButtonEvent::BothHeld]);
        assert!(ui.heating_requested);
        events(&mut ui, &[BACK, ButtonEvent::BothHeld]);
        assert!(!ui.heating_requested);
        for screen in [
            Screen::Menu(MenuItem::Display),
            Screen::Temperature,
            Screen::Measurements,
            Screen::ConfirmDefaults,
        ] {
            ui.screen = screen;
            events(&mut ui, &[BACK, ButtonEvent::BothHeld]);
            assert!(!ui.heating_requested, "{:?}", screen);
        }
    }
}
