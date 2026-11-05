//! Debounced two-button gestures. Holding both suppresses individual actions.
use crate::ui::{Button, ButtonEvent};
const DEBOUNCE_MS: u64 = 25;
const HOLD_MS: u64 = 700;
const CHORD_MS: u64 = 1000;

#[derive(Default)]
pub struct Gestures {
    raw: u8,
    stable: u8,
    raw_since: u64,
    pressed_at: [u64; 2],
    long_sent: u8,
    chord: Option<(u64, bool)>,
}
impl Gestures {
    pub fn idle(&self) -> bool {
        self.raw == 0 && self.stable == 0 && self.chord.is_none()
    }
    pub fn update(&mut self, ms: u64, plus: bool, minus: bool) -> Option<ButtonEvent> {
        let raw = u8::from(plus) | (u8::from(minus) << 1);
        if raw != self.raw {
            self.raw = raw;
            self.raw_since = ms;
        }
        if ms - self.raw_since >= DEBOUNCE_MS && self.stable != raw {
            let old = self.stable;
            self.stable = raw;
            if raw == 3 && self.chord.is_none() {
                self.chord = Some((ms, false));
                return Some(ButtonEvent::BothPressed);
            }
            if self.chord.is_none() {
                for n in 0..2 {
                    let bit = 1 << n;
                    if raw & bit != 0 && old & bit == 0 {
                        self.pressed_at[n] = ms;
                        self.long_sent &= !bit;
                    }
                    if raw & bit == 0 && old & bit != 0 && self.long_sent & bit == 0 {
                        return Some(ButtonEvent::Short(if n == 0 {
                            Button::Plus
                        } else {
                            Button::Minus
                        }));
                    }
                }
            }
        }
        if let Some((start, sent)) = self.chord {
            if self.stable == 0 && self.raw == 0 {
                self.chord = None;
                self.long_sent = 0;
            } else if self.raw != 3 && !sent {
                // Both contacts must remain pressed for the complete hold.
                self.chord = Some((ms, false));
            } else if self.raw == 3 && self.stable == 3 && !sent && ms - start >= CHORD_MS {
                self.chord = Some((start, true));
                return Some(ButtonEvent::BothHeld);
            }
        } else {
            for n in 0..2 {
                let bit = 1 << n;
                if self.raw & self.stable & bit != 0
                    && self.long_sent & bit == 0
                    && ms - self.pressed_at[n] >= HOLD_MS
                {
                    self.long_sent |= bit;
                    return Some(ButtonEvent::Long(if n == 0 {
                        Button::Plus
                    } else {
                        Button::Minus
                    }));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bouncing_edges_do_not_change_settings() {
        let mut g = Gestures::default();
        for ms in 0..100 {
            assert_eq!(g.update(ms, ms % 2 == 0, false), None);
        }
        assert_eq!(g.update(101, false, false), None);
        assert_eq!(g.update(200, false, false), None);
    }
    #[test]
    fn chord_never_emits_individual_short_or_long_presses() {
        let mut g = Gestures::default();
        let mut events = std::vec::Vec::new();
        for ms in 0..3000 {
            let plus = (100..2000).contains(&ms);
            let minus = (200..2100).contains(&ms);
            if let Some(e) = g.update(ms, plus, minus) {
                events.push(e);
            }
        }
        assert_eq!(events, [ButtonEvent::BothPressed, ButtonEvent::BothHeld]);
        assert!(g.idle());
    }
    #[test]
    fn a_long_press_is_not_followed_by_a_short_press() {
        let mut g = Gestures::default();
        let mut event = None;
        for ms in 0..1200 {
            if let Some(e) = g.update(ms, false, (100..1000).contains(&ms)) {
                assert!(event.is_none());
                event = Some(e);
            }
        }
        assert_eq!(event, Some(ButtonEvent::Long(Button::Minus)));
    }
}
