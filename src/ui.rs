//! Two-button navigation and edit transactions, independent of display I/O.

use crate::settings::Settings;
pub use crate::settings::{BRIGHTNESS, MIN_SETPOINT_C, RotationMode};
use crate::{motion::Orientation, settings::Command};

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

/// Each group is a circular list. Typed entries also identify the return point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Temperature,
    Display,
    Measurements,
    Defaults,
    Back,
    Brightness,
    Rotation,
    DisplayBack,
}
impl MenuItem {
    pub const ROOT: [Self; 5] = [
        Self::Temperature,
        Self::Display,
        Self::Measurements,
        Self::Defaults,
        Self::Back,
    ];
    pub const DISPLAY: [Self; 3] = [Self::Brightness, Self::Rotation, Self::DisplayBack];
    pub fn group(self) -> &'static [Self] {
        match self {
            Self::Brightness | Self::Rotation | Self::DisplayBack => &Self::DISPLAY,
            _ => &Self::ROOT,
        }
    }
    pub fn position(self) -> usize {
        self.group().iter().position(|item| *item == self).unwrap()
    }
    fn step(self, button: Button) -> Self {
        let group = self.group();
        let offset = match button {
            Button::Plus => 1,
            Button::Minus => group.len() - 1,
        };
        group[(self.position() + offset) % group.len()]
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Temperature => "TEMPERATURE",
            Self::Display => "DISPLAY",
            Self::Measurements => "MEASUREMENTS",
            Self::Defaults => "DEFAULTS",
            Self::Back | Self::DisplayBack => "BACK",
            Self::Brightness => "BRIGHTNESS",
            Self::Rotation => "ROTATION",
        }
    }
    fn parent(self) -> Screen {
        match self {
            Self::Brightness | Self::Rotation | Self::DisplayBack => Screen::Menu(Self::Display),
            _ => Screen::Main,
        }
    }
}

/// Only the edited field is committed. Concurrent edits to other settings survive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Temperature(u16),
    Brightness(u8),
    Rotation(RotationMode),
}
impl Edit {
    pub fn item(self) -> MenuItem {
        match self {
            Self::Temperature(_) => MenuItem::Temperature,
            Self::Brightness(_) => MenuItem::Brightness,
            Self::Rotation(_) => MenuItem::Rotation,
        }
    }
    fn command(self) -> Command {
        match self {
            Self::Temperature(v) => Command::Temperature(v),
            Self::Brightness(v) => Command::Brightness(v),
            Self::Rotation(v) => Command::Rotation(v),
        }
    }
    fn step(self, button: Button) -> Self {
        match self {
            Self::Temperature(v) => Self::Temperature(match button {
                Button::Plus => (v + 5).min(MAX_SETPOINT_C),
                Button::Minus => v.saturating_sub(5).max(MIN_SETPOINT_C),
            }),
            Self::Brightness(v) => Self::Brightness(match button {
                Button::Plus => (v + 1).min(BRIGHTNESS.len() as u8),
                Button::Minus => v.saturating_sub(1).max(1),
            }),
            Self::Rotation(v) => Self::Rotation(match (v, button) {
                (RotationMode::Auto, Button::Plus) | (RotationMode::Left, Button::Minus) => {
                    RotationMode::Right
                }
                (RotationMode::Right, Button::Plus) | (RotationMode::Auto, Button::Minus) => {
                    RotationMode::Left
                }
                _ => RotationMode::Auto,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Main,
    Menu(MenuItem),
    Edit(Edit),
    Measurements { from_menu: bool },
    ConfirmDefaults,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ui {
    pub screen: Screen,
    /// Committed settings, consumed by control and persistence. Never a draft.
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

    /// A long press never also applies a short press. Every chord stops heat;
    /// only a fresh chord begun on the main screen while OFF may start it.
    pub fn handle(&mut self, event: ButtonEvent) -> bool {
        let previous = *self;
        match event {
            ButtonEvent::BothPressed => {
                self.chord_can_start = self.screen == Screen::Main && !self.heating_requested;
                self.heating_requested = false;
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
                    (Screen::Main, ButtonEvent::Short(button)) => {
                        self.settings.apply(
                            Edit::Temperature(self.settings.setpoint_c)
                                .step(button)
                                .command(),
                        );
                    }
                    (Screen::Main, ButtonEvent::Long(Button::Plus)) => {
                        self.screen = Screen::Menu(MenuItem::Temperature)
                    }
                    (Screen::Main, ButtonEvent::Long(Button::Minus)) => {
                        self.screen = Screen::Measurements { from_menu: false }
                    }
                    (Screen::Menu(item), ButtonEvent::Short(button)) => {
                        self.screen = Screen::Menu(item.step(button))
                    }
                    (Screen::Menu(item), ButtonEvent::Long(Button::Minus)) => {
                        self.screen = item.parent()
                    }
                    (Screen::Menu(item), ButtonEvent::Long(Button::Plus)) => {
                        self.screen = match item {
                            MenuItem::Temperature => {
                                Screen::Edit(Edit::Temperature(self.settings.setpoint_c))
                            }
                            MenuItem::Brightness => {
                                Screen::Edit(Edit::Brightness(self.settings.brightness))
                            }
                            MenuItem::Rotation => {
                                Screen::Edit(Edit::Rotation(self.settings.rotation))
                            }
                            MenuItem::Display => Screen::Menu(MenuItem::Brightness),
                            MenuItem::Measurements => Screen::Measurements { from_menu: true },
                            MenuItem::Defaults => Screen::ConfirmDefaults,
                            MenuItem::Back | MenuItem::DisplayBack => item.parent(),
                        };
                    }
                    (Screen::Edit(edit), ButtonEvent::Short(button)) => {
                        self.screen = Screen::Edit(edit.step(button))
                    }
                    (Screen::Edit(edit), ButtonEvent::Long(Button::Plus)) => {
                        self.settings.apply(edit.command());
                        self.screen = Screen::Menu(edit.item());
                    }
                    (Screen::Edit(edit), ButtonEvent::Long(Button::Minus)) => {
                        self.screen = Screen::Menu(edit.item())
                    }
                    (Screen::Measurements { from_menu }, ButtonEvent::Long(Button::Minus)) => {
                        self.screen = if from_menu {
                            Screen::Menu(MenuItem::Measurements)
                        } else {
                            Screen::Main
                        };
                    }
                    (Screen::Measurements { .. }, ButtonEvent::Long(Button::Plus)) => {
                        self.screen = Screen::Menu(MenuItem::Measurements)
                    }
                    (Screen::ConfirmDefaults, ButtonEvent::Long(Button::Plus)) => {
                        self.heating_requested = false;
                        self.settings.apply(Command::Defaults);
                        self.screen = Screen::Menu(MenuItem::Defaults);
                    }
                    (Screen::ConfirmDefaults, ButtonEvent::Long(Button::Minus)) => {
                        self.screen = Screen::Menu(MenuItem::Defaults)
                    }
                    _ => {}
                }
            }
        }
        *self != previous
    }

    /// Remote edits cancel a conflicting draft instead of silently overwriting it.
    pub fn apply_settings(&mut self, command: Command) -> bool {
        let previous = *self;
        // Invalid input must not dismiss an editor. A valid no-op command
        // still cancels its conflicting draft (the remote value wins).
        if matches!(command, Command::Temperature(v) if !(MIN_SETPOINT_C..=MAX_SETPOINT_C).contains(&v))
            || matches!(command, Command::Brightness(v) if !(1..=BRIGHTNESS.len() as u8).contains(&v))
        {
            return false;
        }
        self.settings.apply(command);
        if let Screen::Edit(edit) = self.screen {
            let same_field = matches!(
                (edit, command),
                (_, Command::Defaults)
                    | (Edit::Temperature(_), Command::Temperature(_))
                    | (Edit::Brightness(_), Command::Brightness(_))
                    | (Edit::Rotation(_), Command::Rotation(_))
            );
            if same_field {
                self.screen = Screen::Menu(edit.item());
            }
        }
        *self != previous
    }

    pub fn contrast(&self) -> u8 {
        let brightness = match self.screen {
            Screen::Edit(Edit::Brightness(v)) => v,
            _ => self.settings.brightness,
        };
        Settings {
            brightness,
            ..self.settings
        }
        .contrast()
    }
    pub fn display_orientation(&self, detected: Orientation) -> Orientation {
        let mode = match self.screen {
            Screen::Edit(Edit::Rotation(v)) => v,
            _ => self.settings.rotation,
        };
        mode.orientation(detected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const PLUS: ButtonEvent = ButtonEvent::Short(Button::Plus);
    const MINUS: ButtonEvent = ButtonEvent::Short(Button::Minus);
    const ENTER: ButtonEvent = ButtonEvent::Long(Button::Plus);
    const BACK: ButtonEvent = ButtonEvent::Long(Button::Minus);
    fn events(ui: &mut Ui, events: &[ButtonEvent]) {
        for &event in events {
            ui.handle(event);
        }
    }

    #[test]
    fn setpoint_steps_are_bounded_in_both_directions() {
        let mut ui = Ui::default();
        for _ in 0..100 {
            ui.handle(PLUS);
        }
        assert_eq!(ui.settings.setpoint_c, MAX_SETPOINT_C);
        assert!(!ui.handle(PLUS));
        for _ in 0..100 {
            ui.handle(MINUS);
        }
        assert_eq!(ui.settings.setpoint_c, MIN_SETPOINT_C);
        assert!(!ui.handle(MINUS));
    }
    #[test]
    fn nested_navigation_wraps_and_returns_to_its_parent_selection() {
        let mut ui = Ui::default();
        events(&mut ui, &[ENTER, MINUS]);
        assert_eq!(ui.screen, Screen::Menu(MenuItem::Back));
        events(&mut ui, &[PLUS, PLUS, ENTER, MINUS]);
        assert_eq!(ui.screen, Screen::Menu(MenuItem::DisplayBack));
        ui.handle(ENTER);
        assert_eq!(ui.screen, Screen::Menu(MenuItem::Display));
        ui.handle(BACK);
        assert_eq!(ui.screen, Screen::Main);
    }
    #[test]
    fn drafts_are_previewed_but_only_confirmed_values_are_persistable() {
        let mut ui = Ui::default();
        let original = ui.settings;
        events(&mut ui, &[ENTER, PLUS, ENTER, ENTER, PLUS]);
        assert_eq!(ui.contrast(), 64);
        assert_eq!(ui.settings, original);
        ui.handle(BACK);
        assert_eq!(ui.contrast(), 26);
        events(&mut ui, &[ENTER, PLUS, ENTER]);
        assert_eq!(ui.settings.brightness, 3);
        assert_eq!(ui.screen, Screen::Menu(MenuItem::Brightness));
        events(&mut ui, &[PLUS, ENTER, PLUS]);
        assert_eq!(
            ui.display_orientation(Orientation::Left),
            Orientation::Right
        );
        assert_eq!(ui.settings.rotation, RotationMode::Auto);
        ui.handle(BACK);
        assert_eq!(ui.display_orientation(Orientation::Left), Orientation::Left);
    }
    #[test]
    fn editing_temperature_cannot_change_the_running_target_before_confirmation() {
        let mut ui = Ui::default();
        events(&mut ui, &[ENTER, ENTER, PLUS]);
        assert_eq!(ui.settings.setpoint_c, 150);
        ui.handle(ENTER);
        assert_eq!(ui.settings.setpoint_c, 155);
        assert!(!ui.heating_requested);
    }
    #[test]
    fn editor_limits_keep_the_display_visible() {
        let mut ui = Ui::default();
        events(&mut ui, &[ENTER, PLUS, ENTER, ENTER]);
        for _ in 0..100 {
            ui.handle(MINUS);
        }
        assert_eq!(ui.contrast(), 8);
        assert!(!ui.handle(MINUS));
        for _ in 0..100 {
            ui.handle(PLUS);
        }
        assert_eq!(ui.contrast(), 101);
        assert!(!ui.handle(PLUS));
    }
    #[test]
    fn measurements_return_to_their_entry_point() {
        let mut ui = Ui::default();
        ui.handle(BACK);
        assert_eq!(ui.screen, Screen::Measurements { from_menu: false });
        assert!(!ui.handle(PLUS));
        ui.handle(BACK);
        assert_eq!(ui.screen, Screen::Main);
        events(&mut ui, &[ENTER, PLUS, PLUS, ENTER, BACK]);
        assert_eq!(ui.screen, Screen::Menu(MenuItem::Measurements));
    }
    #[test]
    fn defaults_require_confirmation_and_stop_heating() {
        let mut ui = Ui::with_settings(Settings {
            setpoint_c: 240,
            ..Settings::default()
        });
        ui.heating_requested = true;
        events(&mut ui, &[ENTER, MINUS, MINUS, ENTER, BACK]);
        assert_eq!(ui.settings.setpoint_c, 240);
        assert!(ui.heating_requested);
        events(&mut ui, &[ENTER, ENTER]);
        assert_eq!(ui.settings, Settings::default());
        assert!(!ui.heating_requested);
    }
    #[test]
    fn chords_stop_everywhere_but_can_only_start_on_main() {
        let mut ui = Ui::default();
        assert!(!ui.handle(ButtonEvent::BothHeld));
        events(&mut ui, &[ButtonEvent::BothPressed, ButtonEvent::BothHeld]);
        assert!(ui.heating_requested);
        ui.handle(ENTER);
        events(&mut ui, &[ButtonEvent::BothPressed, ButtonEvent::BothHeld]);
        assert!(!ui.heating_requested);
        events(&mut ui, &[BACK, ButtonEvent::BothHeld]);
        assert!(!ui.heating_requested);
        events(
            &mut ui,
            &[
                ButtonEvent::BothPressed,
                ButtonEvent::BothHeld,
                ButtonEvent::BothPressed,
                ButtonEvent::BothHeld,
            ],
        );
        assert!(!ui.heating_requested);
    }
    #[test]
    fn remote_edits_preserve_unrelated_fields_and_cancel_conflicting_drafts() {
        let mut ui = Ui::default();
        events(&mut ui, &[ENTER, ENTER, PLUS]);
        ui.apply_settings(Command::Brightness(4));
        ui.handle(ENTER);
        assert_eq!(ui.settings.setpoint_c, 155);
        assert_eq!(ui.settings.brightness, 4);
        events(&mut ui, &[ENTER, PLUS]);
        ui.apply_settings(Command::Temperature(200));
        assert_eq!(ui.screen, Screen::Menu(MenuItem::Temperature));
        assert_eq!(ui.settings.setpoint_c, 200);
        events(&mut ui, &[ENTER, PLUS]);
        assert!(!ui.apply_settings(Command::Temperature(0)));
        assert_eq!(ui.screen, Screen::Edit(Edit::Temperature(205)));
        assert!(ui.apply_settings(Command::Temperature(200)));
        assert_eq!(ui.screen, Screen::Menu(MenuItem::Temperature));
    }
}
