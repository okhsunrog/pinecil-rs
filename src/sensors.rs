//! Pinecil V2 analog conversions. These are nominal estimates, not calibration.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorFault {
    Timeout,
    Saturated,
    Adc,
    OutOfRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reading<T> {
    Pending,
    Value(T),
    Fault(SensorFault),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measurements {
    pub sampled_at_ms: u64,
    pub input_mv: Reading<u32>,
    pub handle_centi_c: Reading<i32>,
    pub tip_centi_c: Reading<i32>,
    /// Gain-corrected ADC counts in VIN / tip / handle order, or None on failure.
    pub counts: [Option<u16>; 3],
    pub gain_calibrated: bool,
    pub adc_errors: u32,
}

impl Measurements {
    pub const WAITING: Self = Self {
        sampled_at_ms: 0,
        input_mv: Reading::Pending,
        handle_centi_c: Reading::Pending,
        tip_centi_c: Reading::Pending,
        counts: [None; 3],
        gain_calibrated: false,
        adc_errors: 0,
    };
}

// Production schematic R10/R11 and R25/R26, in ohms. ADC reference and the
// NTC's 3.3 V supply are nominal; neither is measured by this implementation.
const VIN_TOP_OHMS: u32 = 120_000;
const VIN_BOTTOM_OHMS: u32 = 10_000;
const TIP_FEEDBACK_OHMS: u32 = 680_000;
const TIP_GAIN_OHMS: u32 = 2_370;
const ADC_REFERENCE_UV: u32 = 3_200_000;
const TIP_SATURATION_UV: u32 = 3_150_000;
const TIP_SHORT_UV: u32 = 10_000;

/// Starting value used by the Pinecil V2 BSP, not this cartridge's calibration.
pub const DEFAULT_TIP_OFFSET_UV: u32 = 900;

// NTCG163JF103FTDS, 10 kohm pull-down, 3.3 V supply. Numerical reference
// points from IronOS's Pinecil V2 NTC model, expressed on a 3.2 V / 65536
// scale. TDK specifies R25=10 kohm, B25/50=3380 K; see docs/measurements.md.
// The points cover -40..125 C in 5 C increments. Out-of-range inputs fail.
const NTC_COUNTS: [u32; 34] = [
    3405, 4380, 5572, 6999, 8688, 10650, 12885, 15384, 18129, 21074, 24172, 27362, 30595, 33792,
    36907, 39891, 42704, 45325, 47736, 49929, 51912, 53689, 55274, 56679, 57923, 59020, 59984,
    60832, 61580, 62232, 62810, 63316, 63765, 64158,
];

// Numerical reference curve used by the Pinecil V2 BSP, 0..500 C in 10 C
// increments. It is a cartridge model, not a generic type-K thermocouple.
const TIP_UV: [u32; 51] = [
    0, 266, 522, 770, 1010, 1244, 1473, 1697, 1917, 2135, 2351, 2566, 2780, 2994, 3209, 3426, 3644,
    3865, 4088, 4314, 4544, 4777, 5014, 5255, 5500, 5750, 6003, 6261, 6523, 6789, 7059, 7332, 7609,
    7889, 8171, 8456, 8742, 9030, 9319, 9607, 9896, 10183, 10468, 10750, 11029, 11304, 11573,
    11835, 12091, 12337, 12575,
];

fn interpolate(points: &[u32], input: u32, first: i32, step: i32) -> Reading<i32> {
    if input < points[0] || input > points[points.len() - 1] {
        return Reading::Fault(SensorFault::OutOfRange);
    }
    for (i, pair) in points.windows(2).enumerate() {
        if input <= pair[1] {
            return Reading::Value(
                first
                    + i as i32 * step
                    + ((input - pair[0]) * step as u32 / (pair[1] - pair[0])) as i32,
            );
        }
    }
    unreachable!()
}

/// Input rail voltage from the 120 kohm / 10 kohm divider on GPIO18.
pub fn input_voltage_mv(pin_uv: u32) -> Reading<u32> {
    if pin_uv > ADC_REFERENCE_UV {
        return Reading::Fault(SensorFault::OutOfRange);
    }
    Reading::Value(
        (u64::from(pin_uv) * u64::from(VIN_TOP_OHMS + VIN_BOTTOM_OHMS)
            / u64::from(VIN_BOTTOM_OHMS)
            / 1000) as u32,
    )
}

/// Handle temperature in centidegrees C; rejects open/short/out-of-range NTCs.
pub fn handle_temperature_centi_c(pin_uv: u32) -> Reading<i32> {
    if pin_uv > ADC_REFERENCE_UV {
        return Reading::Fault(SensorFault::OutOfRange);
    }
    let scaled = (u64::from(pin_uv) * 65536 / u64::from(ADC_REFERENCE_UV)) as u32;
    interpolate(&NTC_COUNTS, scaled, -4000, 500)
}

/// Tip estimate with nominal amplifier gain, offset and handle compensation.
/// A rail-high reading is unavailable: it can mean an absent cartridge, a wiring
/// fault or excessive temperature. It never becomes a plausible temperature.
pub fn tip_temperature_centi_c(pin_uv: u32, handle: Reading<i32>, offset_uv: u32) -> Reading<i32> {
    if pin_uv >= TIP_SATURATION_UV {
        return Reading::Fault(SensorFault::Saturated);
    }
    if pin_uv <= TIP_SHORT_UV {
        return Reading::Fault(SensorFault::OutOfRange);
    }
    let handle_centi_c = match handle {
        Reading::Value(value) => value,
        Reading::Pending => return Reading::Pending,
        Reading::Fault(error) => return Reading::Fault(error),
    };
    let thermocouple_uv = (u64::from(pin_uv) * u64::from(TIP_GAIN_OHMS)
        / u64::from(TIP_GAIN_OHMS + TIP_FEEDBACK_OHMS)) as u32;
    match interpolate(&TIP_UV, thermocouple_uv.saturating_sub(offset_uv), 0, 1000) {
        Reading::Value(delta) => Reading::Value(delta + handle_centi_c),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voltage_divider_covers_usb_and_epr_and_rejects_invalid_input() {
        assert_eq!(input_voltage_mv(0), Reading::Value(0));
        assert_eq!(input_voltage_mv(400_000), Reading::Value(5200));
        assert_eq!(input_voltage_mv(2_000_000), Reading::Value(26_000));
        assert_eq!(input_voltage_mv(2_154_000), Reading::Value(28_002));
        assert_eq!(
            input_voltage_mv(3_200_001),
            Reading::Fault(SensorFault::OutOfRange)
        );
    }

    #[test]
    fn nominal_ntc_at_twenty_five_and_faults_at_both_rails() {
        assert_eq!(handle_temperature_centi_c(1_650_000), Reading::Value(2500));
        assert_eq!(
            handle_temperature_centi_c(0),
            Reading::Fault(SensorFault::OutOfRange)
        );
        assert_eq!(
            handle_temperature_centi_c(3_200_000),
            Reading::Fault(SensorFault::OutOfRange)
        );
    }

    #[test]
    fn ntc_curve_is_monotonic_and_hits_reference_points() {
        for (i, &counts) in NTC_COUNTS.iter().enumerate() {
            assert_eq!(
                interpolate(&NTC_COUNTS, counts, -4000, 500),
                Reading::Value(-4000 + i as i32 * 500)
            );
        }
        assert!(NTC_COUNTS.windows(2).all(|p| p[0] < p[1]));
        assert!(TIP_UV.windows(2).all(|p| p[0] < p[1]));
    }

    #[test]
    fn tip_uses_handle_compensation_and_does_not_hide_a_saturated_input() {
        assert_eq!(
            tip_temperature_centi_c(259_128, Reading::Value(2500), 900),
            Reading::Value(2500)
        );
        // 2351 uV corresponds to 100 C of tip rise in the reference curve.
        let pin_uv = (2351 + 900) * (TIP_GAIN_OHMS + TIP_FEEDBACK_OHMS) / TIP_GAIN_OHMS + 1;
        assert_eq!(
            tip_temperature_centi_c(pin_uv, Reading::Value(2500), 900),
            Reading::Value(12_500)
        );
        assert_eq!(
            tip_temperature_centi_c(3_150_000, Reading::Value(2500), 900),
            Reading::Fault(SensorFault::Saturated)
        );
        assert_eq!(
            tip_temperature_centi_c(0, Reading::Value(2500), 900),
            Reading::Fault(SensorFault::OutOfRange)
        );
        assert_eq!(
            tip_temperature_centi_c(259_128, Reading::Fault(SensorFault::Adc), 900),
            Reading::Fault(SensorFault::Adc)
        );
    }
}
