//! Sole ADC/heater owner. Temperature control uses timer-limited carrier bursts.

use crate::board::BoardAdc;
use crate::heater_io::HeaterOutput;
use bl706_hal::{
    adc::{Channel, Error, Sample},
    gpio::Output,
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};
use embassy_time::{Duration, Instant, Ticker, Timer};
use pinecil_rs::sensors::{self, Measurements, Reading, SensorFault};
use pinecil_rs::{
    app::StateReceiver,
    heater::{self, Controller, Fault, Request, ResistanceFault},
};

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

struct ProbeGuard<'a>(&'a mut Output<24>);
impl Drop for ProbeGuard<'_> {
    fn drop(&mut self) {
        self.0.set_low();
    }
}

async fn probe(
    adc: &mut BoardAdc,
    pin: &mut Output<24>,
    watchdog: &mut bl706_hal::watchdog::Watchdog,
) -> Result<u32, ResistanceFault> {
    let pin = ProbeGuard(pin);
    pin.0.set_low();
    let mut readings = [0; 3];
    for (index, value) in readings.iter_mut().enumerate() {
        Timer::after_millis(100).await;
        watchdog.feed();
        let (_, uv) = sample(adc, Channel::Input9)
            .await
            .map_err(|_| ResistanceFault::Adc)?;
        *value = sensors::thermocouple_uv(uv);
        if index == 0 {
            pin.0.set_high();
        } else {
            pin.0.set_low();
        }
    }
    let result = heater::classify_resistance(readings);
    log::info!(
        "Tip resistance probe: off/on/off uV={:?}, mOhm={:?}",
        readings,
        result
    );
    result
}

#[embassy_executor::task]
pub async fn run(
    mut adc: BoardAdc,
    output: HeaterOutput,
    mut tip_probe: Output<24>,
    mut watchdog: bl706_hal::watchdog::Watchdog,
    mut snapshots: StateReceiver<'static, CriticalSectionRawMutex>,
    timer_ok: bool,
) {
    let gain_calibrated = adc.config().calibration.is_some();
    let mut adc_errors = 0u32;
    let mut last_adc_error = None;
    let mut controller = Controller::default();
    watchdog.start();
    let mut resistance = probe(&mut adc, &mut tip_probe, &mut watchdog).await;
    let mut ticker = Ticker::every(Duration::from_micros(u64::from(heater::PERIOD_US)));
    let mut next_log = Instant::now();
    let mut was_enabled = false;
    #[cfg(feature = "heater-diagnostics")]
    let mut test = crate::heater_test::Test::default();
    log::info!(
        "Tip controller: starts OFF, max={}mW, ADC trim={}, cutoff test={}",
        heater::MAX_POWER_MW,
        gain_calibrated,
        timer_ok
    );
    loop {
        let previous_output = output.stats();
        output.stop();
        if previous_output.active {
            controller.trip(Fault::Timing);
        }
        Timer::after_micros(heater::SETTLE_US).await;
        let acquired_at_ms = Instant::now().as_millis();
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
        frame.sampled_at_ms = acquired_at_ms;
        frame.adc_errors = adc_errors;
        if !matches!(frame.tip_centi_c, Reading::Value(_)) {
            resistance = Err(ResistanceFault::Open);
        }
        let state = snapshots.try_get();
        let request = state.map_or(Request::OFF, |state| Request {
            enabled: state.ui.heating_requested,
            target_c: state.ui.settings.setpoint_c,
            limit_mw: heater::MAX_POWER_MW,
        });
        #[cfg(feature = "heater-diagnostics")]
        let request = {
            let request = test.update(request);
            if core::mem::take(&mut test.inject_sensor_fault) {
                frame.tip_centi_c = Reading::Fault(SensorFault::Adc);
            }
            if core::mem::take(&mut test.test_watchdog) {
                output.stop();
                tip_probe.set_low();
                log::info!("Heater watchdog test: output OFF, withholding feed");
                loop {
                    core::hint::spin_loop();
                }
            }
            if core::mem::take(&mut test.stall_task) {
                // No new burst is armed; the previous one ended in the ISR.
                let until = Instant::now() + Duration::from_millis(350);
                while Instant::now() < until {
                    core::hint::spin_loop();
                }
            }
            request
        };
        // Apply the maintenance interlock after diagnostic overrides as well.
        let request = if crate::storage::BUSY.load(bl706_hal::portable_atomic::Ordering::Acquire) {
            Request::OFF
        } else {
            request
        };
        let starting = request.enabled && !was_enabled;
        was_enabled = request.enabled;
        if starting
            || !request.enabled
                && resistance.is_err()
                && matches!(frame.tip_centi_c, Reading::Value(_))
        {
            resistance = probe(&mut adc, &mut tip_probe, &mut watchdog).await;
            // Probe current invalidates this frame: sample again with it low.
            frame.heater = controller.update(
                Instant::now().as_millis(),
                Request::OFF,
                &frame,
                pinecil_rs::power::PowerStatus::Waiting,
                resistance,
            );
            MEASUREMENTS.sender().send(frame);
            watchdog.feed();
            ticker.reset();
            continue;
        }
        frame.heater = controller.update(
            Instant::now().as_millis(),
            request,
            &frame,
            crate::pd::STATUS
                .try_get()
                .unwrap_or(pinecil_rs::power::PowerStatus::Waiting),
            resistance,
        );
        if request.enabled && !timer_ok {
            frame.heater = controller.trip(Fault::Output);
        }
        if request.enabled && previous_output.max_late_us > 2000 {
            frame.heater = controller.trip(Fault::Timing);
        }
        if frame.heater.burst_us > 0 && output.burst(frame.heater.burst_us, true).is_err() {
            output.stop();
            frame.heater = controller.trip(Fault::Output);
        }
        watchdog.feed();
        MEASUREMENTS.sender().send(frame);
        if Instant::now() >= next_log {
            log::info!(
                "sensors ms={} VIN_mV={:?} handle_cC={:?} tip_cC={:?} counts={:?} trim={} errors={} last_error={:?}",
                frame.sampled_at_ms,
                frame.input_mv,
                frame.handle_centi_c,
                frame.tip_centi_c,
                frame.counts,
                gain_calibrated,
                adc_errors,
                last_adc_error
            );
            log::info!(
                "heater {:?} target_C={} R_mOhm={:?} est_mW={} burst_us={} cutoff={:?}",
                frame.heater.state,
                frame.heater.target_c,
                frame.heater.resistance_mohm,
                frame.heater.power_mw,
                frame.heater.burst_us,
                output.stats()
            );
            next_log = Instant::now() + Duration::from_secs(1);
        }
        ticker.next().await;
    }
}
