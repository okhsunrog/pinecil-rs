//! Application state and event processing, independent of display and GPIO I/O.

use embassy_futures::select::{Either3, select3};
use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    channel::Channel,
    watch::{Receiver, Watch},
};

use crate::{
    power::PowerStatus,
    sensors::Measurements,
    ui::{ButtonEvent, Ui},
};

pub type ButtonEvents<M> = Channel<M, ButtonEvent, 8>;
// Display, control and telemetry may each subscribe without consuming another
// reader's update. Only the display is implemented at present.
pub type StateWatch<M> = Watch<M, AppState, 3>;
pub type StateReceiver<'a, M> = Receiver<'a, M, AppState, 3>;

/// The latest application snapshot. Negotiated power is not an ADC measurement.
/// Changing the setpoint alone never enables heating; a two-button hold does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppState {
    pub ui: Ui,
    pub power: PowerStatus,
    pub measurements: Measurements,
}

/// Owns UI state and consumes input even while the display is busy or offline.
/// Publishing replaces the previous snapshot without waiting for any reader.
/// Intermediate states may be skipped; button events remain ordered in a queue.
pub async fn run<M: RawMutex, const N: usize, const K: usize>(
    buttons: &ButtonEvents<M>,
    mut power: Receiver<'_, M, PowerStatus, N>,
    mut measurements: Receiver<'_, M, Measurements, K>,
    snapshots: &StateWatch<M>,
) -> ! {
    let mut state = AppState {
        ui: Ui::default(),
        // Read the latest contract even if PD initialized before this task.
        power: power.get().await,
        measurements: measurements.get().await,
    };
    let sender = snapshots.sender();
    sender.send(state);

    loop {
        let previous = state;
        // Observe ready power updates before draining queued button events.
        match select3(power.changed(), measurements.changed(), buttons.receive()).await {
            Either3::First(status) => state.power = status,
            Either3::Second(frame) => state.measurements = frame,
            Either3::Third(event) => {
                state.ui.handle(event);
            }
        }
        if state != previous {
            sender.send(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{Button, Screen};
    use core::{
        future::Future,
        pin::{Pin, pin},
        task::{Context, Waker},
    };
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    fn drive(future: Pin<&mut impl Future>) {
        assert!(
            future
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
    }

    #[test]
    fn stalled_display_does_not_block_buttons_or_power_updates() {
        let buttons = ButtonEvents::<NoopRawMutex>::new();
        let power = Watch::<NoopRawMutex, _, 3>::new_with(PowerStatus::Waiting);
        let measurements = Watch::<NoopRawMutex, _, 3>::new_with(Measurements::WAITING);
        let snapshots = StateWatch::new();
        let mut display = snapshots.receiver().unwrap();
        let mut application = pin!(run(
            &buttons,
            power.receiver().unwrap(),
            measurements.receiver().unwrap(),
            &snapshots
        ));
        drive(application.as_mut());

        // Do not poll the display while more events than the queue can hold
        // are processed. An OLED recovery must not stall application input.
        for _ in 0..20 {
            buttons.try_send(ButtonEvent::Short(Button::Plus)).unwrap();
            drive(application.as_mut());
        }
        buttons.try_send(ButtonEvent::Long(Button::Plus)).unwrap();
        drive(application.as_mut());
        buttons.try_send(ButtonEvent::Short(Button::Plus)).unwrap();
        power.sender().send(PowerStatus::Contract {
            mv: 28_000,
            ma: 3_000,
        });
        drive(application.as_mut());

        let state = display.try_get().unwrap();
        assert_eq!(state.ui.setpoint_c, crate::ui::MAX_SETPOINT_C);
        assert_eq!(state.ui.screen, Screen::Brightness);
        assert_eq!(state.ui.brightness_level(), 3);
        assert_eq!(
            state.power,
            PowerStatus::Contract {
                mv: 28_000,
                ma: 3_000
            }
        );
    }

    #[test]
    fn independent_readers_and_late_subscriber_observe_latest_state() {
        let buttons = ButtonEvents::<NoopRawMutex>::new();
        let power = Watch::<NoopRawMutex, _, 3>::new_with(PowerStatus::Contract {
            mv: 20_000,
            ma: 1_500,
        });
        let measurements = Watch::<NoopRawMutex, _, 3>::new_with(Measurements::WAITING);
        let snapshots = StateWatch::new();
        let mut display = snapshots.receiver().unwrap();
        let mut telemetry = snapshots.receiver().unwrap();
        let mut application = pin!(run(
            &buttons,
            power.receiver().unwrap(),
            measurements.receiver().unwrap(),
            &snapshots
        ));
        drive(application.as_mut());
        assert_eq!(display.try_get().unwrap().power, power.try_get().unwrap());

        buttons.try_send(ButtonEvent::Short(Button::Plus)).unwrap();
        drive(application.as_mut());
        power.sender().send(PowerStatus::Waiting);
        drive(application.as_mut());
        power.sender().send(PowerStatus::Error);
        drive(application.as_mut());

        let latest = display.try_changed().unwrap();
        assert_eq!(latest.ui.setpoint_c, 305);
        assert_eq!(latest.power, PowerStatus::Error);
        assert_eq!(telemetry.try_changed(), Some(latest));
        let mut late = snapshots.receiver().unwrap();
        assert_eq!(late.try_get(), Some(latest));
    }

    #[test]
    fn ignored_input_does_not_request_a_redraw() {
        let buttons = ButtonEvents::<NoopRawMutex>::new();
        let power = Watch::<NoopRawMutex, _, 1>::new_with(PowerStatus::Waiting);
        let measurements = Watch::<NoopRawMutex, _, 3>::new_with(Measurements::WAITING);
        let snapshots = StateWatch::new();
        let mut display = snapshots.receiver().unwrap();
        let mut application = pin!(run(
            &buttons,
            power.receiver().unwrap(),
            measurements.receiver().unwrap(),
            &snapshots
        ));
        drive(application.as_mut());
        display.try_get().unwrap();

        for _ in 0..30 {
            buttons.try_send(ButtonEvent::Short(Button::Plus)).unwrap();
            drive(application.as_mut());
        }
        display.try_get().unwrap();
        buttons.try_send(ButtonEvent::Short(Button::Plus)).unwrap();
        power.sender().send(PowerStatus::Waiting);
        drive(application.as_mut());
        assert!(display.try_changed().is_none());
    }

    #[test]
    fn sensor_fault_replaces_old_temperature_without_losing_other_state() {
        use crate::sensors::{Reading, SensorFault};

        let buttons = ButtonEvents::<NoopRawMutex>::new();
        let contract = PowerStatus::Contract {
            mv: 28_000,
            ma: 3_000,
        };
        let power = Watch::<NoopRawMutex, _, 1>::new_with(contract);
        let measurements = Watch::<NoopRawMutex, _, 1>::new_with(Measurements::WAITING);
        let snapshots = StateWatch::new();
        let mut display = snapshots.receiver().unwrap();
        let mut application = pin!(run(
            &buttons,
            power.receiver().unwrap(),
            measurements.receiver().unwrap(),
            &snapshots
        ));
        drive(application.as_mut());
        let good = Measurements {
            sampled_at_ms: 1,
            input_mv: Reading::Value(28_000),
            handle_centi_c: Reading::Value(2500),
            tip_centi_c: Reading::Value(2500),
            ..Measurements::WAITING
        };
        measurements.sender().send(good);
        buttons.try_send(ButtonEvent::Short(Button::Plus)).unwrap();
        drive(application.as_mut());
        assert_eq!(display.try_get().unwrap().measurements, good);

        // Keep the display idle while a later sample reports a tip failure.
        let failed = Measurements {
            sampled_at_ms: 2,
            tip_centi_c: Reading::Fault(SensorFault::Saturated),
            adc_errors: 1,
            ..good
        };
        measurements.sender().send(failed);
        drive(application.as_mut());
        let latest = display.try_changed().unwrap();
        assert_eq!(latest.measurements, failed);
        assert_eq!(latest.ui.setpoint_c, 305);
        assert_eq!(latest.power, contract);
    }
}
