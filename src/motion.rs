//! Board motion policy for raw SC7A20 samples in HR, ±2 g mode.
//! Thresholds are raw output words, not a calibrated acceleration conversion.

const ROTATION_THRESHOLD: i32 = 8_000;
const ROTATION_HOLD_MS: u64 = 400;
const MOVEMENT_THRESHOLD: i32 = 1_200;
pub const SAMPLE_STALE_MS: u64 = 500;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Orientation {
    #[default]
    Right,
    Left,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Starting,
    Ready,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MotionState {
    pub status: Status,
    pub orientation: Orientation,
    pub raw: Option<[i16; 3]>,
    pub sampled_at_ms: u64,
    pub last_movement_at_ms: Option<u64>,
}

impl MotionState {
    pub const WAITING: Self = Self {
        status: Status::Starting,
        orientation: Orientation::Right,
        raw: None,
        sampled_at_ms: 0,
        last_movement_at_ms: None,
    };

    /// Unavailable/stale measurements cannot establish that the iron is idle.
    pub fn idle_ms(&self, now_ms: u64) -> Option<u64> {
        if self.status != Status::Ready
            || now_ms.saturating_sub(self.sampled_at_ms) > SAMPLE_STALE_MS
        {
            return None;
        }
        self.last_movement_at_ms
            .map(|last| now_ms.saturating_sub(last))
    }
}

pub struct Tracker {
    state: MotionState,
    filtered: Option<[i32; 3]>,
    candidate: Option<(Orientation, u64)>,
}

impl Default for Tracker {
    fn default() -> Self {
        Self {
            state: MotionState::WAITING,
            filtered: None,
            candidate: None,
        }
    }
}

impl Tracker {
    pub fn observe(&mut self, raw: [i16; 3], now_ms: u64) -> MotionState {
        let sample = raw.map(i32::from);
        if now_ms.saturating_sub(self.state.sampled_at_ms) > SAMPLE_STALE_MS {
            self.filtered = None;
            self.candidate = None;
        }
        let filtered = match self.filtered {
            None => {
                // Start a new observation period after boot or sensor recovery.
                self.state.last_movement_at_ms = Some(now_ms);
                sample
            }
            Some(mut baseline) => {
                let delta: i32 = sample
                    .iter()
                    .zip(baseline)
                    .map(|(value, previous)| (value - previous).abs())
                    .sum();
                if delta >= MOVEMENT_THRESHOLD {
                    self.state.last_movement_at_ms = Some(now_ms);
                }
                // Quarter-step low-pass filter. i32 also handles i16 extremes.
                for (value, new) in baseline.iter_mut().zip(sample) {
                    *value += (new - *value) / 4;
                }
                baseline
            }
        };
        self.filtered = Some(filtered);

        // Pinecil's native SC7A20 uses Y for left/right roll, as in IronOS.
        // Hold the previous rotation when flat or pointing along the X axis.
        let [x, y, _] = filtered;
        // A current sample must still support the filtered candidate. Without
        // this check, filter lag could finish a hold after a brief roll ended.
        let current_supports = sample[1].abs() >= ROTATION_THRESHOLD
            && sample[1].abs() > sample[0].abs()
            && sample[1].signum() == y.signum();
        let wanted = if current_supports && y.abs() >= ROTATION_THRESHOLD && y.abs() > x.abs() {
            Some(if y > 0 {
                Orientation::Left
            } else {
                Orientation::Right
            })
        } else {
            None
        };
        match wanted {
            Some(orientation) if orientation != self.state.orientation => {
                if let Some((previous, since)) = self.candidate
                    && previous == orientation
                {
                    if now_ms.saturating_sub(since) >= ROTATION_HOLD_MS {
                        self.state.orientation = orientation;
                        self.candidate = None;
                    }
                } else {
                    self.candidate = Some((orientation, now_ms));
                }
            }
            _ => self.candidate = None,
        }
        self.state.status = Status::Ready;
        self.state.raw = Some(raw);
        self.state.sampled_at_ms = now_ms;
        self.state
    }

    /// Forget timing/filter history but retain the last stable orientation.
    pub fn unavailable(&mut self) -> MotionState {
        self.filtered = None;
        self.candidate = None;
        self.state.status = Status::Unavailable;
        self.state.raw = None;
        self.state.last_movement_at_ms = None;
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_requires_a_continuous_hold_and_ignores_flat_pose() {
        let mut tracker = Tracker::default();
        for time in (0..400).step_by(40) {
            assert_eq!(
                tracker.observe([0, 16_000, 0], time).orientation,
                Orientation::Right
            );
        }
        assert_eq!(
            tracker.observe([0, 16_000, 0], 400).orientation,
            Orientation::Left
        );
        for time in (440..4000).step_by(40) {
            assert_eq!(
                tracker.observe([0, 0, -16_000], time).orientation,
                Orientation::Left
            );
        }
        for time in (4000..6000).step_by(40) {
            tracker.observe([0, -16_000, 0], time);
        }
        assert_eq!(tracker.state.orientation, Orientation::Right);
    }

    #[test]
    fn short_roll_and_vertical_pose_do_not_flip_the_screen() {
        let mut tracker = Tracker::default();
        for time in (0..320).step_by(40) {
            tracker.observe([0, 16_000, 0], time);
        }
        for time in (320..2000).step_by(40) {
            tracker.observe([16_000, 9_000, 0], time);
        }
        assert_eq!(tracker.state.orientation, Orientation::Right);
    }

    #[test]
    fn stillness_accumulates_but_movement_and_recovery_restart_it() {
        let mut tracker = Tracker::default();
        for time in (0..=2000).step_by(40) {
            let state = tracker.observe([0, 0, -16_000], time);
            assert_eq!(state.idle_ms(time), Some(time));
        }
        let state = tracker.observe([2_000, 0, -16_000], 2040);
        assert_eq!(state.idle_ms(2040), Some(0));
        assert_eq!(state.idle_ms(2541), None);
        assert_eq!(tracker.unavailable().idle_ms(2560), None);
        assert_eq!(
            tracker.observe([2_000, 0, -16_000], 3000).idle_ms(3000),
            Some(0)
        );
    }

    #[test]
    fn sensor_gap_cannot_complete_a_pending_rotation() {
        let mut tracker = Tracker::default();
        tracker.observe([0, 16_000, 0], 0);
        assert_eq!(
            tracker.observe([0, 16_000, 0], 1000).orientation,
            Orientation::Right
        );
        tracker.unavailable();
        assert_eq!(
            tracker.observe([0, 16_000, 0], 2000).orientation,
            Orientation::Right
        );
    }

    #[test]
    fn full_scale_extremes_do_not_overflow_and_fault_retains_orientation() {
        let mut tracker = Tracker::default();
        for time in (0..=400).step_by(40) {
            tracker.observe([0, i16::MAX, 0], time);
        }
        assert_eq!(tracker.unavailable().orientation, Orientation::Left);
        tracker.observe([i16::MIN; 3], 1000);
        tracker.observe([i16::MAX; 3], 1040);
    }
}
