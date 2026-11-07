//! Application state and event processing, independent of display and GPIO I/O.

use embassy_futures::select::{Either5, select5};
use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    channel::Channel,
    watch::{Receiver, Watch},
};

use crate::{
    motion::{MotionState, Orientation},
    power::PowerStatus,
    sensors::Measurements,
    ui::{ButtonEvent, Ui},
};

pub type SettingsCommands<M> = Channel<M, crate::settings::Command, 4>;
pub type ButtonEvents<M> = Channel<M, ButtonEvent, 8>;
// Display, control and storage subscribe independently without consuming
// another reader's update.
pub type StateWatch<M> = Watch<M, AppState, 3>;
pub type StateReceiver<'a, M> = Receiver<'a, M, AppState, 3>;

/// The latest application snapshot. Negotiated power is not an ADC measurement.
/// Changing the setpoint alone never enables heating; a two-button hold does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppState {
    pub ui: Ui,
    pub power: PowerStatus,
    pub measurements: Measurements,
    pub motion: MotionState,
}

impl AppState {
    pub fn display_orientation(&self) -> Orientation {
        self.ui.display_orientation(self.motion.orientation)
    }
}

/// Owns UI state and consumes input even while the display is busy or offline.
/// Publishing replaces the previous snapshot without waiting for any reader.
/// Intermediate states may be skipped; button events remain ordered in a queue.
pub async fn run<M: RawMutex, const N: usize, const K: usize, const L: usize>(
    buttons: &ButtonEvents<M>,
    mut power: Receiver<'_, M, PowerStatus, N>,
    mut measurements: Receiver<'_, M, Measurements, K>,
    mut motion: Receiver<'_, M, MotionState, L>,
    snapshots: &StateWatch<M>,
    commands: &SettingsCommands<M>,
    initial_settings: crate::settings::Settings,
) -> ! {
    let mut state = AppState {
        ui: Ui::with_settings(initial_settings),
        // Read the latest contract even if PD initialized before this task.
        power: power.get().await,
        measurements: measurements.get().await,
        motion: motion.get().await,
    };
    let sender = snapshots.sender();
    sender.send(state);

    loop {
        let previous = state;
        // Observe ready power updates before draining queued button events.
        match select5(
            power.changed(),
            measurements.changed(),
            motion.changed(),
            buttons.receive(),
            commands.receive(),
        )
        .await
        {
            Either5::First(status) => state.power = status,
            Either5::Second(frame) => state.measurements = frame,
            Either5::Third(frame) => state.motion = frame,
            Either5::Fourth(event) => {
                state.ui.handle(event);
            }
            Either5::Fifth(command) => {
                state.ui.apply_settings(command);
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
    use crate::ui::{Button, MenuItem, Screen};
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
    fn restored_settings_and_remote_edits_never_restore_heat_enable() {
        let buttons = ButtonEvents::<NoopRawMutex>::new();
        let commands = SettingsCommands::new();
        let power = Watch::<NoopRawMutex, _, 1>::new_with(PowerStatus::Waiting);
        let measurements = Watch::<NoopRawMutex, _, 1>::new_with(Measurements::WAITING);
        let motion = Watch::<NoopRawMutex, _, 1>::new_with(MotionState::WAITING);
        let snapshots = StateWatch::new();
        let initial = crate::settings::Settings {
            setpoint_c: 230,
            brightness: 4,
            rotation: crate::settings::RotationMode::Left,
        };
        let mut application = pin!(run(
            &buttons,
            power.receiver().unwrap(),
            measurements.receiver().unwrap(),
            motion.receiver().unwrap(),
            &snapshots,
            &commands,
            initial
        ));
        drive(application.as_mut());
        assert_eq!(snapshots.try_get().unwrap().ui.settings, initial);
        assert!(!snapshots.try_get().unwrap().ui.heating_requested);
        commands
            .try_send(crate::settings::Command::Temperature(240))
            .unwrap();
        drive(application.as_mut());
        let state = snapshots.try_get().unwrap();
        assert_eq!(state.ui.settings.setpoint_c, 240);
        assert!(!state.ui.heating_requested);

        // Storage consumes the same snapshots: editing a draft changes the
        // screen, but must leave the persisted/controller settings untouched.
        for event in [
            ButtonEvent::Long(Button::Plus),
            ButtonEvent::Long(Button::Plus),
            ButtonEvent::Short(Button::Plus),
        ] {
            buttons.try_send(event).unwrap();
            drive(application.as_mut());
        }
        let draft = snapshots.try_get().unwrap();
        assert_eq!(draft.ui.settings.setpoint_c, 240);
        assert_eq!(
            draft.ui.screen,
            Screen::Edit(crate::ui::Edit::Temperature(245))
        );
        buttons.try_send(ButtonEvent::Long(Button::Plus)).unwrap();
        drive(application.as_mut());
        let applied = snapshots.try_get().unwrap();
        assert_eq!(applied.ui.settings.setpoint_c, 245);
        assert!(!applied.ui.heating_requested);
    }

    #[test]
    fn stalled_display_does_not_block_buttons_or_power_updates() {
        let buttons = ButtonEvents::<NoopRawMutex>::new();
        let commands = SettingsCommands::new();
        let power = Watch::<NoopRawMutex, _, 3>::new_with(PowerStatus::Waiting);
        let measurements = Watch::<NoopRawMutex, _, 3>::new_with(Measurements::WAITING);
        let motion = Watch::<NoopRawMutex, _, 1>::new_with(MotionState::WAITING);
        let snapshots = StateWatch::new();
        let mut display = snapshots.receiver().unwrap();
        let mut application = pin!(run(
            &buttons,
            power.receiver().unwrap(),
            measurements.receiver().unwrap(),
            motion.receiver().unwrap(),
            &snapshots,
            &commands,
            crate::settings::Settings::default()
        ));
        drive(application.as_mut());

        // Do not poll the display while more events than the queue can hold
        // are processed. An OLED recovery must not stall application input.
        for _ in 0..crate::ui::MAX_SETPOINT_C {
            buttons.try_send(ButtonEvent::Short(Button::Plus)).unwrap();
            drive(application.as_mut());
        }
        buttons.try_send(ButtonEvent::Long(Button::Plus)).unwrap();
        drive(application.as_mut());
        for event in [
            ButtonEvent::Short(Button::Plus), // Display
            ButtonEvent::Long(Button::Plus),  // Brightness
            ButtonEvent::Long(Button::Plus),  // edit
            ButtonEvent::Short(Button::Plus),
            ButtonEvent::Long(Button::Plus), // apply
        ] {
            buttons.try_send(event).unwrap();
            drive(application.as_mut());
        }
        power.sender().send(PowerStatus::Contract {
            mv: 28_000,
            ma: 3_000,
        });
        drive(application.as_mut());

        let state = display.try_get().unwrap();
        assert_eq!(state.ui.settings.setpoint_c, crate::ui::MAX_SETPOINT_C);
        assert_eq!(state.ui.screen, Screen::Menu(MenuItem::Brightness));
        assert_eq!(usize::from(state.ui.settings.brightness), 3);
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
        let commands = SettingsCommands::new();
        let power = Watch::<NoopRawMutex, _, 3>::new_with(PowerStatus::Contract {
            mv: 20_000,
            ma: 1_500,
        });
        let measurements = Watch::<NoopRawMutex, _, 3>::new_with(Measurements::WAITING);
        let motion = Watch::<NoopRawMutex, _, 1>::new_with(MotionState::WAITING);
        let snapshots = StateWatch::new();
        let mut display = snapshots.receiver().unwrap();
        let mut telemetry = snapshots.receiver().unwrap();
        let mut application = pin!(run(
            &buttons,
            power.receiver().unwrap(),
            measurements.receiver().unwrap(),
            motion.receiver().unwrap(),
            &snapshots,
            &commands,
            crate::settings::Settings::default()
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
        assert_eq!(latest.ui.settings.setpoint_c, 155);
        assert_eq!(latest.power, PowerStatus::Error);
        assert_eq!(telemetry.try_changed(), Some(latest));
        let mut late = snapshots.receiver().unwrap();
        assert_eq!(late.try_get(), Some(latest));
    }

    #[test]
    fn ignored_input_does_not_request_a_redraw() {
        let buttons = ButtonEvents::<NoopRawMutex>::new();
        let commands = SettingsCommands::new();
        let power = Watch::<NoopRawMutex, _, 1>::new_with(PowerStatus::Waiting);
        let measurements = Watch::<NoopRawMutex, _, 3>::new_with(Measurements::WAITING);
        let motion = Watch::<NoopRawMutex, _, 1>::new_with(MotionState::WAITING);
        let snapshots = StateWatch::new();
        let mut display = snapshots.receiver().unwrap();
        let mut application = pin!(run(
            &buttons,
            power.receiver().unwrap(),
            measurements.receiver().unwrap(),
            motion.receiver().unwrap(),
            &snapshots,
            &commands,
            crate::settings::Settings::default()
        ));
        drive(application.as_mut());
        display.try_get().unwrap();

        for _ in 0..crate::ui::MAX_SETPOINT_C {
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
        let commands = SettingsCommands::new();
        let contract = PowerStatus::Contract {
            mv: 28_000,
            ma: 3_000,
        };
        let power = Watch::<NoopRawMutex, _, 1>::new_with(contract);
        let measurements = Watch::<NoopRawMutex, _, 1>::new_with(Measurements::WAITING);
        let motion = Watch::<NoopRawMutex, _, 1>::new_with(MotionState::WAITING);
        let snapshots = StateWatch::new();
        let mut display = snapshots.receiver().unwrap();
        let mut application = pin!(run(
            &buttons,
            power.receiver().unwrap(),
            measurements.receiver().unwrap(),
            motion.receiver().unwrap(),
            &snapshots,
            &commands,
            crate::settings::Settings::default()
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
        assert_eq!(latest.ui.settings.setpoint_c, 155);
        assert_eq!(latest.power, contract);
    }

    #[test]
    fn motion_and_sensor_loss_never_enable_heating_or_override_manual_rotation() {
        use crate::{motion::Status, ui::RotationMode};

        let buttons = ButtonEvents::<NoopRawMutex>::new();
        let commands = SettingsCommands::new();
        let power = Watch::<NoopRawMutex, _, 1>::new_with(PowerStatus::Waiting);
        let measurements = Watch::<NoopRawMutex, _, 1>::new_with(Measurements::WAITING);
        let motion = Watch::<NoopRawMutex, _, 1>::new_with(MotionState::WAITING);
        let snapshots = StateWatch::new();
        let mut display = snapshots.receiver().unwrap();
        let mut application = pin!(run(
            &buttons,
            power.receiver().unwrap(),
            measurements.receiver().unwrap(),
            motion.receiver().unwrap(),
            &snapshots,
            &commands,
            crate::settings::Settings::default()
        ));
        drive(application.as_mut());
        let sample = MotionState {
            status: Status::Ready,
            orientation: Orientation::Left,
            raw: Some([0, 16_000, 0]),
            sampled_at_ms: 400,
            last_movement_at_ms: Some(400),
        };
        motion.sender().send(sample);
        drive(application.as_mut());
        let state = display.try_get().unwrap();
        assert_eq!(state.display_orientation(), Orientation::Left);
        assert!(!state.ui.heating_requested);

        for event in [
            ButtonEvent::Long(Button::Plus),  // root
            ButtonEvent::Short(Button::Plus), // Display
            ButtonEvent::Long(Button::Plus),  // Brightness
            ButtonEvent::Short(Button::Plus), // Rotation
            ButtonEvent::Long(Button::Plus),  // edit
            ButtonEvent::Short(Button::Plus), // Right
            ButtonEvent::Long(Button::Plus),  // apply
        ] {
            buttons.try_send(event).unwrap();
            drive(application.as_mut());
        }
        motion.sender().send(MotionState {
            status: Status::Unavailable,
            raw: None,
            last_movement_at_ms: None,
            ..sample
        });
        drive(application.as_mut());
        let state = display.try_get().unwrap();
        assert_eq!(state.ui.settings.rotation, RotationMode::Right);
        assert_eq!(state.display_orientation(), Orientation::Right);
        assert!(!state.ui.heating_requested);
        assert_eq!(state.motion.idle_ms(1000), None);
    }
}
