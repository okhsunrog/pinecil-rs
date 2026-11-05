//! Read-only tip controller. Owns ADC and GPIO21; PWM is never routed or enabled.

use crate::board::BoardAdc;
use bl706_hal::{
    adc::{Channel, Error, Sample},
    gpio::{Input, Output},
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};
use embassy_time::{Duration, Instant, Ticker, Timer};
use pinecil_rs::sensors::{self, Measurements, Reading, SensorFault};

pub static MEASUREMENTS: Watch<CriticalSectionRawMutex, Measurements, 3> =
    Watch::new_with(Measurements::WAITING);

/// Read repeated samples of one input, independently of other channels. A
/// saturated tip must not discard the VIN/NTC readings from the same cycle.
async fn sample(adc: &mut BoardAdc, channel: Channel) -> Result<(u16, u32), Error> {
    let mut samples = [Sample::default(); 4];
    adc.read_scan(&[channel; 4], &mut samples, Duration::from_millis(25))
        .await?;
    let counts = (samples.iter().map(|s| u32::from(s.counts)).sum::<u32>() / 4) as u16;
    let mut averaged = samples[0];
    averaged.counts = counts;
    let config = adc.config();
    Ok((
        counts,
        averaged.microvolts(config.resolution, config.reference),
    ))
}

fn fault(error: Error) -> SensorFault {
    match error {
        Error::Timeout => SensorFault::Timeout,
        Error::Saturated => SensorFault::Saturated,
        _ => SensorFault::Adc,
    }
}

#[embassy_executor::task]
pub async fn run(mut adc: BoardAdc, mut heater_off: Output<21>, _tip_probe: Input<24>) {
    let gain_calibrated = adc.config().calibration.is_some();
    let mut adc_errors = 0u32;
    let mut last_adc_error = None;
    let mut ticker = Ticker::every(Duration::from_millis(250));
    let mut next_log = Instant::now();
    log::info!(
        "Read-only tip controller: heater OFF, ADC trim={}",
        gain_calibrated
    );
    loop {
        heater_off.set_low();
        // Reserve a settling interval with the heater off before every scan.
        // Future PWM control must preserve a qualified measurement window.
        Timer::after_millis(1).await;
        let mut frame = Measurements {
            gain_calibrated,
            ..Measurements::WAITING
        };
        // Handle precedes tip because it supplies cold-junction compensation.
        for channel in [Channel::Input8, Channel::Input10, Channel::Input9] {
            let slot = match channel {
                Channel::Input8 => 0,  // VIN
                Channel::Input9 => 1,  // tip
                Channel::Input10 => 2, // handle
                _ => unreachable!(),
            };
            match sample(&mut adc, channel).await {
                Ok((counts, uv)) => {
                    frame.counts[slot] = Some(counts);
                    match channel {
                        Channel::Input8 => frame.input_mv = sensors::input_voltage_mv(uv),
                        Channel::Input10 => {
                            frame.handle_centi_c = sensors::handle_temperature_centi_c(uv)
                        }
                        Channel::Input9 => {
                            frame.tip_centi_c = sensors::tip_temperature_centi_c(
                                uv,
                                frame.handle_centi_c,
                                sensors::DEFAULT_TIP_OFFSET_UV,
                            )
                        }
                        _ => unreachable!(),
                    }
                }
                Err(error) => {
                    adc_errors = adc_errors.saturating_add(1);
                    last_adc_error = Some(error);
                    log::debug!("ADC {:?}: {:?}", channel, error);
                    let reading = fault(error);
                    match channel {
                        Channel::Input8 => frame.input_mv = Reading::Fault(reading),
                        Channel::Input10 => frame.handle_centi_c = Reading::Fault(reading),
                        Channel::Input9 => frame.tip_centi_c = Reading::Fault(reading),
                        _ => unreachable!(),
                    }
                }
            }
        }
        heater_off.set_low();
        frame.sampled_at_ms = Instant::now().as_millis();
        frame.adc_errors = adc_errors;
        MEASUREMENTS.sender().send(frame);
        if Instant::now() >= next_log {
            log::info!(
                "sensors ms={} VIN_mV={:?} handle_cC={:?} tip_cC={:?} counts={:?} trim={} errors={} last_error={:?} heater=OFF",
                frame.sampled_at_ms,
                frame.input_mv,
                frame.handle_centi_c,
                frame.tip_centi_c,
                frame.counts,
                gain_calibrated,
                adc_errors,
                last_adc_error
            );
            next_log = Instant::now() + Duration::from_secs(1);
        }
        ticker.next().await;
    }
}
