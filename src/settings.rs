//! Persistent user settings. Heating enable and transient UI state are excluded.
use crate::motion::Orientation;
use serde::{Deserialize, Serialize};

pub const MIN_SETPOINT_C: u16 = 150;
pub const BRIGHTNESS: [u8; 4] = [8, 26, 64, 101];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RotationMode {
    #[default]
    Auto,
    Right,
    Left,
}
impl RotationMode {
    pub fn orientation(self, detected: Orientation) -> Orientation {
        match self {
            Self::Auto => detected,
            Self::Right => Orientation::Right,
            Self::Left => Orientation::Left,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "AUTO",
            Self::Right => "RIGHT",
            Self::Left => "LEFT",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub setpoint_c: u16,
    pub brightness: u8,
    pub rotation: RotationMode,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            setpoint_c: 150,
            brightness: 2,
            rotation: RotationMode::Auto,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Temperature(u16),
    Brightness(u8),
    Rotation(RotationMode),
    Defaults,
}

impl Settings {
    pub fn valid(self) -> bool {
        (MIN_SETPOINT_C..=crate::heater::MAX_TARGET_C).contains(&self.setpoint_c)
            && (1..=BRIGHTNESS.len() as u8).contains(&self.brightness)
    }
    pub fn apply(&mut self, command: Command) -> bool {
        let mut next = *self;
        match command {
            Command::Temperature(v) => next.setpoint_c = v,
            Command::Brightness(v) => next.brightness = v,
            Command::Rotation(v) => next.rotation = v,
            Command::Defaults => next = Self::default(),
        }
        if !next.valid() {
            return false;
        }
        let changed = *self != next;
        *self = next;
        changed
    }
    pub fn contrast(self) -> u8 {
        BRIGHTNESS
            .get(usize::from(self.brightness.saturating_sub(1)))
            .copied()
            .unwrap_or(BRIGHTNESS[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_updates_leave_the_previous_settings_intact() {
        let mut s = Settings::default();
        let before = s;
        for c in [
            Command::Temperature(0),
            Command::Temperature(351),
            Command::Brightness(0),
            Command::Brightness(5),
        ] {
            assert!(!s.apply(c));
            assert_eq!(s, before);
        }
        assert!(s.apply(Command::Temperature(175)));
        assert!(s.apply(Command::Brightness(4)));
        assert!(s.apply(Command::Rotation(RotationMode::Left)));
        assert!(s.apply(Command::Defaults));
        assert_eq!(s, before);
    }
}
