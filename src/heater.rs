//! Hardware-independent temperature control and heater interlocks.
//!
//! The controller requests energy in bounded PWM-carrier bursts. A separate
//! timer ISR must end each burst; a new ADC frame is required before rearming.
use crate::{
    power::PowerStatus,
    sensors::{Measurements, Reading},
};

pub const PERIOD_US: u32 = 100_000;
pub const MAX_BURST_US: u32 = 75_000;
pub const MIN_BURST_US: u32 = 500;
pub const SETTLE_US: u64 = 10_000;
pub const MAX_SAMPLE_AGE_MS: u64 = 150;
pub const MAX_CONTROL_GAP_MS: u64 = 250;
/// Initial hardware qualification limit, independent of the PD contract.
pub const MAX_POWER_MW: u32 = 20_000;
pub const MAX_TARGET_C: u16 = 350;
const SUPPLY_MARGIN_MA: u32 = 200;
const MAX_HANDLE_CENTI_C: i32 = 6000;
const MAX_TIP_CENTI_C: i32 = 42_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResistanceFault {
    Pending,
    Adc,
    Open,
    Short,
    Unstable,
}

/// Probe-voltage bins used by the Pinecil V2 BSP, not a precision ohmmeter.
pub fn classify_resistance(thermocouple_uv: [u32; 3]) -> Result<u32, ResistanceFault> {
    let [before, driven, after] = thermocouple_uv.map(i64::from);
    if (before - after).abs() > 200 {
        return Err(ResistanceFault::Unstable);
    }
    let rise = driven - (before + after) / 2;
    match rise {
        ..500 => Err(ResistanceFault::Short),
        500..2600 => Ok(4000),
        2600..4000 => Ok(6200),
        4000..=8000 => Ok(8000),
        _ => Err(ResistanceFault::Open),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Sensor,
    StaleSample,
    Timing,
    NoTip,
    NoPower,
    Voltage,
    HandleHot,
    TipHot,
    NoTemperatureRise,
    Output,
}
impl Fault {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sensor => "ADC",
            Self::StaleSample => "STALE",
            Self::Timing => "TIMING",
            Self::NoTip => "TIP",
            Self::NoPower => "POWER",
            Self::Voltage => "VIN",
            Self::HandleHot => "HANDLE",
            Self::TipHot => "HOT",
            Self::NoTemperatureRise => "NO RISE",
            Self::Output => "OUTPUT",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Off,
    Heating,
    Regulating,
    Fault(Fault),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    pub target_c: u16,
    pub resistance_mohm: Result<u32, ResistanceFault>,
    /// Estimated average electrical power, not a measured current/power value.
    pub power_mw: u32,
    pub burst_us: u32,
}
impl Status {
    pub const OFF: Self = Self {
        state: State::Off,
        target_c: 0,
        resistance_mohm: Err(ResistanceFault::Pending),
        power_mw: 0,
        burst_us: 0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub enabled: bool,
    pub target_c: u16,
    pub limit_mw: u32,
}
impl Request {
    pub const OFF: Self = Self {
        enabled: false,
        target_c: 150,
        limit_mw: MAX_POWER_MW,
    };
}

/// Convert average requested power to an on-window using P = V²/R. The
/// carrier keeps the AC-coupled gate driver active; its 50% electrical duty
/// is not an extra 0.5 multiplier on the cartridge's on-window power.
pub fn burst_for_power(power_mw: u32, vin_mv: u32, resistance_mohm: u32) -> u32 {
    if !(9000..=30_000).contains(&vin_mv) || !(4000..=8000).contains(&resistance_mohm) {
        return 0;
    }
    let full_mw = u64::from(vin_mv) * u64::from(vin_mv) / u64::from(resistance_mohm);
    let burst = (u64::from(power_mw.min(MAX_POWER_MW)) * u64::from(PERIOD_US) / full_mw)
        .min(u64::from(MAX_BURST_US)) as u32;
    if burst < MIN_BURST_US { 0 } else { burst }
}

/// PI with conditional integration at the power limits. Gains are conservative
/// starting values in W/°C and W/(°C·s), not copied JBC or IronOS integer gains.
struct Pi {
    integral_w: f32,
}
impl Pi {
    fn update(&mut self, error_c: f32, dt_s: f32, limit_w: f32) -> f32 {
        self.integral_w = self.integral_w.clamp(0.0, limit_w);
        let proportional = error_c * 0.35;
        let increment = error_c * dt_s * 0.10;
        let candidate = proportional + self.integral_w + increment;
        if (0.0..=limit_w).contains(&candidate)
            || candidate > limit_w && increment < 0.0
            || candidate < 0.0 && increment > 0.0
        {
            self.integral_w = (self.integral_w + increment).clamp(0.0, limit_w);
        }
        (proportional + self.integral_w).clamp(0.0, limit_w)
    }
}

pub struct Controller {
    pi: Pi,
    status: Status,
    last_sample_ms: Option<u64>,
    target_c: u16,
    progress: Option<(u64, i32)>,
    energy_mj: u64,
}
impl Default for Controller {
    fn default() -> Self {
        Self {
            pi: Pi { integral_w: 0.0 },
            status: Status::OFF,
            last_sample_ms: None,
            target_c: 0,
            progress: None,
            energy_mj: 0,
        }
    }
}
impl Controller {
    pub fn trip(&mut self, fault: Fault) -> Status {
        self.status.state = State::Fault(fault);
        self.status.power_mw = 0;
        self.status.burst_us = 0;
        self.pi.integral_w = 0.0;
        self.status
    }

    /// Faults latch until an explicit OFF request. A repeated, stale or invalid
    /// frame never reuses the preceding heating command.
    pub fn update(
        &mut self,
        now_ms: u64,
        request: Request,
        frame: &Measurements,
        power: PowerStatus,
        resistance: Result<u32, ResistanceFault>,
    ) -> Status {
        self.status.resistance_mohm = resistance;
        self.status.target_c = request.target_c.clamp(50, MAX_TARGET_C);
        self.status.power_mw = 0;
        self.status.burst_us = 0;
        if !request.enabled {
            self.pi.integral_w = 0.0;
            self.status.state = State::Off;
            self.last_sample_ms = None;
            self.progress = None;
            self.energy_mj = 0;
            return self.status;
        }
        if matches!(self.status.state, State::Fault(_)) {
            return self.status;
        }
        if frame.sampled_at_ms > now_ms || now_ms - frame.sampled_at_ms > MAX_SAMPLE_AGE_MS {
            return self.trip(Fault::StaleSample);
        }
        let elapsed_ms = match self.last_sample_ms {
            Some(previous) if frame.sampled_at_ms <= previous => {
                return self.trip(Fault::StaleSample);
            }
            Some(previous) if frame.sampled_at_ms - previous > MAX_CONTROL_GAP_MS => {
                return self.trip(Fault::Timing);
            }
            Some(previous) => frame.sampled_at_ms - previous,
            None => 100,
        };
        self.last_sample_ms = Some(frame.sampled_at_ms);
        let (Reading::Value(vin), Reading::Value(handle), Reading::Value(tip)) =
            (frame.input_mv, frame.handle_centi_c, frame.tip_centi_c)
        else {
            return self.trip(Fault::Sensor);
        };
        if !frame.gain_calibrated {
            return self.trip(Fault::Sensor);
        }
        if !(0..MAX_HANDLE_CENTI_C).contains(&handle) {
            return self.trip(Fault::HandleHot);
        }
        if !(0..MAX_TIP_CENTI_C).contains(&tip) {
            return self.trip(Fault::TipHot);
        }
        let Ok(resistance) = resistance else {
            return self.trip(Fault::NoTip);
        };
        if ![4000, 6200, 8000].contains(&resistance) {
            return self.trip(Fault::NoTip);
        }
        let PowerStatus::Contract { mv, ma } = power else {
            return self.trip(Fault::NoPower);
        };
        if !(9000..=28_000).contains(&mv) || ma <= SUPPLY_MARGIN_MA {
            return self.trip(Fault::NoPower);
        }
        if vin.abs_diff(mv) > mv / 10 || !(9000..=30_000).contains(&vin) {
            return self.trip(Fault::Voltage);
        }
        let target = request.target_c.clamp(50, MAX_TARGET_C);
        if target != self.target_c {
            self.pi.integral_w = 0.0;
            self.progress = None;
            self.energy_mj = 0;
            self.target_c = target;
        }
        let limit_mw = (u64::from(vin) * u64::from(ma - SUPPLY_MARGIN_MA) / 1000)
            .min(
                u64::from(vin) * u64::from(vin) / u64::from(resistance) * u64::from(MAX_BURST_US)
                    / u64::from(PERIOD_US),
            )
            .min(u64::from(request.limit_mw.min(MAX_POWER_MW))) as u32;
        let error_c = f32::from(target) - tip as f32 / 100.0;
        let desired = (self.pi.update(
            error_c,
            elapsed_ms as f32 / 1000.0,
            limit_mw as f32 / 1000.0,
        ) * 1000.0) as u32;
        let burst = burst_for_power(desired, vin, resistance);
        let actual_mw = (u64::from(vin) * u64::from(vin) / u64::from(resistance) * u64::from(burst)
            / u64::from(PERIOD_US)) as u32;
        if error_c > 10.0 && actual_mw > 0 {
            let (started, initial_centi) = *self.progress.get_or_insert((now_ms, tip));
            self.energy_mj += u64::from(actual_mw) * u64::from(PERIOD_US) / 1_000_000;
            if tip >= initial_centi + 300 {
                self.progress = Some((now_ms, tip));
                self.energy_mj = 0;
            } else if now_ms - started >= 5000 && self.energy_mj >= 20_000 {
                return self.trip(Fault::NoTemperatureRise);
            }
        } else {
            self.progress = None;
            self.energy_mj = 0;
        }
        self.status.state = if error_c.abs() <= 5.0 {
            State::Regulating
        } else {
            State::Heating
        };
        self.status.burst_us = burst;
        self.status.power_mw = actual_mw;
        self.status
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::SensorFault;
    fn frame(ms: u64, temp: i32) -> Measurements {
        Measurements {
            sampled_at_ms: ms,
            input_mv: Reading::Value(28_000),
            handle_centi_c: Reading::Value(2500),
            tip_centi_c: Reading::Value(temp),
            gain_calibrated: true,
            ..Measurements::WAITING
        }
    }
    const REQUEST: Request = Request {
        enabled: true,
        target_c: 150,
        limit_mw: 5000,
    };
    const POWER: PowerStatus = PowerStatus::Contract {
        mv: 28_000,
        ma: 3000,
    };
    #[test]
    fn resistance_probe_removes_offset_and_linear_thermal_drift() {
        assert_eq!(classify_resistance([600, 3940, 680]), Ok(6200));
        assert_eq!(classify_resistance([600, 5100, 600]), Ok(8000));
        assert_eq!(
            classify_resistance([600, 600, 600]),
            Err(ResistanceFault::Short)
        );
        assert_eq!(
            classify_resistance([600, 9600, 600]),
            Err(ResistanceFault::Open)
        );
        assert_eq!(
            classify_resistance([600, 4000, 1200]),
            Err(ResistanceFault::Unstable)
        );
    }
    #[test]
    fn power_to_burst_respects_average_energy_and_measurement_window() {
        for vin in [9000, 12_000, 20_000, 28_000] {
            for resistance in [4000, 6200, 8000] {
                for request in [0, 1, 500, 5000, 20_000, u32::MAX] {
                    let burst = burst_for_power(request, vin, resistance);
                    assert!(burst <= MAX_BURST_US);
                    assert!(burst == 0 || burst >= MIN_BURST_US);
                    let average = u64::from(vin) * u64::from(vin) / u64::from(resistance)
                        * u64::from(burst)
                        / u64::from(PERIOD_US);
                    assert!(average <= u64::from(request.min(MAX_POWER_MW)));
                }
            }
        }
        assert_eq!(burst_for_power(20_000, 0, 8000), 0);
        assert_eq!(burst_for_power(20_000, 28_000, 0), 0);
    }
    #[test]
    fn bad_samples_and_contract_loss_cut_output_and_latch() {
        for fault_case in 0..7 {
            let mut c = Controller::default();
            assert!(
                c.update(100, REQUEST, &frame(100, 2500), POWER, Ok(6200))
                    .burst_us
                    > 0
            );
            let mut bad = frame(200, 2500);
            let mut power = POWER;
            let mut resistance = Ok(6200);
            match fault_case {
                0 => bad.tip_centi_c = Reading::Fault(SensorFault::Saturated),
                1 => bad.handle_centi_c = Reading::Value(6500),
                2 => bad.gain_calibrated = false,
                3 => bad.input_mv = Reading::Value(5000),
                4 => power = PowerStatus::Waiting,
                5 => resistance = Err(ResistanceFault::Open),
                _ => bad.tip_centi_c = Reading::Value(43_000),
            }
            let failed = c.update(200, REQUEST, &bad, power, resistance);
            assert!(matches!(failed.state, State::Fault(_)));
            assert_eq!(failed.burst_us, 0);
            assert_eq!(
                c.update(300, REQUEST, &frame(300, 2500), POWER, Ok(6200))
                    .state,
                failed.state
            );
            assert_eq!(
                c.update(400, Request::OFF, &bad, power, resistance).state,
                State::Off
            );
        }
    }
    #[test]
    fn stale_repeated_future_and_delayed_samples_never_rearm() {
        for sample_ms in [100, 99, 500] {
            let mut c = Controller::default();
            c.update(100, REQUEST, &frame(100, 2500), POWER, Ok(8000));
            assert_eq!(
                c.update(400, REQUEST, &frame(sample_ms, 2500), POWER, Ok(8000))
                    .burst_us,
                0
            );
        }
        let mut c = Controller::default();
        c.update(100, REQUEST, &frame(100, 2500), POWER, Ok(8000));
        assert_eq!(
            c.update(400, REQUEST, &frame(400, 2500), POWER, Ok(8000))
                .state,
            State::Fault(Fault::Timing)
        );
    }
    #[test]
    fn stuck_thermocouple_trips_even_if_adc_frames_remain_fresh() {
        let mut c = Controller::default();
        let mut status = Status::OFF;
        for ms in (100..=6000).step_by(100) {
            status = c.update(ms, REQUEST, &frame(ms, 2500), POWER, Ok(8000));
        }
        assert_eq!(status.state, State::Fault(Fault::NoTemperatureRise));
        assert_eq!(status.power_mw, 0);
    }
    #[test]
    fn anti_windup_allows_immediate_off_after_reaching_target() {
        let mut pi = Pi { integral_w: 0.0 };
        for _ in 0..1000 {
            assert_eq!(pi.update(200.0, 0.1, 5.0), 5.0);
        }
        assert_eq!(pi.integral_w, 0.0);
        assert_eq!(pi.update(-10.0, 0.1, 5.0), 0.0);
    }

    #[test]
    fn simulated_thermal_load_tracks_target_across_a_supply_change() {
        // Deliberately simple independent plant: 1 J/K thermal capacity and
        // 0.05 W/K heat loss. This checks the complete power/control contract,
        // not the unmeasured physical dynamics of a Pinecil cartridge.
        let mut c = Controller::default();
        let request = Request {
            limit_mw: MAX_POWER_MW,
            ..REQUEST
        };
        let mut temperature = 25.0f32;
        let mut peak = temperature;
        for tick in 1..=1200 {
            let vin = if tick < 600 { 20_000 } else { 28_000 };
            let mut f = frame(tick * 100, (temperature * 100.0) as i32);
            f.input_mv = Reading::Value(vin);
            let power = PowerStatus::Contract { mv: vin, ma: 1500 };
            let result = c.update(tick * 100, request, &f, power, Ok(8000));
            assert!(!matches!(result.state, State::Fault(_)));
            assert!(result.power_mw <= MAX_POWER_MW);
            temperature += (result.power_mw as f32 / 1000.0 - 0.05 * (temperature - 25.0)) * 0.1;
            peak = peak.max(temperature);
        }
        assert!(
            (145.0..155.0).contains(&temperature),
            "temperature={temperature}"
        );
        assert!(peak < 165.0, "peak={peak}");
    }
}
