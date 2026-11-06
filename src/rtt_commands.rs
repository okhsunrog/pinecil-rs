//! Operator commands over RTT; no UART or extra GPIO wiring is required.
use embassy_time::Timer;

fn execute(line: &[u8]) {
    let Ok(command) = core::str::from_utf8(line) else {
        log::warn!("RTT command must be UTF-8");
        return;
    };
    let command = command.trim();
    if command == "settings status" {
        if let Some(status) = crate::storage::STATUS.try_get() {
            log::info!("Settings {:?}; saved={:?}", status.phase, status.saved);
            log::info!("Flash {:?}", status.metrics);
        }
        log::info!(
            "Settings active={:?}",
            crate::APP_STATE.try_get().map(|s| s.ui.settings)
        );
        return;
    }
    #[cfg(feature = "storage-diagnostics")]
    if command == "settings stress" {
        crate::storage::STRESS.store(true, bl706_hal::portable_atomic::Ordering::Release);
        log::info!("Settings GC stress queued; requires heater OFF and SPR");
        return;
    }
    if let Some(argument) = command.strip_prefix("settings ") {
        use pinecil_rs::settings::{Command, RotationMode};
        let update = match argument {
            "defaults" => Some(Command::Defaults),
            "rotate auto" => Some(Command::Rotation(RotationMode::Auto)),
            "rotate left" => Some(Command::Rotation(RotationMode::Left)),
            "rotate right" => Some(Command::Rotation(RotationMode::Right)),
            _ => argument
                .strip_prefix("temp ")
                .and_then(|s| s.parse().ok())
                .map(Command::Temperature)
                .or_else(|| {
                    argument
                        .strip_prefix("bright ")
                        .and_then(|s| s.parse().ok())
                        .map(Command::Brightness)
                }),
        };
        if let Some(update) = update {
            let valid = match update {
                Command::Temperature(t) => (pinecil_rs::settings::MIN_SETPOINT_C
                    ..=pinecil_rs::heater::MAX_TARGET_C)
                    .contains(&t),
                Command::Brightness(b) => (1..=4).contains(&b),
                _ => true,
            };
            if valid {
                log::info!(
                    "Settings command {:?}: {:?}",
                    update,
                    crate::storage::COMMANDS.try_send(update)
                );
            } else {
                log::warn!("Invalid settings value");
            }
        } else {
            log::warn!(
                "Settings: status | temp 150..350 | bright 1..4 | rotate auto/right/left | defaults"
            );
        }
        return;
    }
    match command {
        "pd spr" => {
            let ui_off = crate::APP_STATE
                .try_get()
                .is_some_and(|s| !s.ui.heating_requested);
            let output_off = crate::control::MEASUREMENTS
                .try_get()
                .is_some_and(|s| s.heater.state == pinecil_rs::heater::State::Off);
            if !ui_off || !output_off {
                log::warn!("PD SPR command rejected: switch heating OFF first");
                return;
            }
            crate::pd::request_spr();
            log::info!("PD SPR requested; negotiation pending");
        }
        "pd status" => log::info!(
            "PD status: spr_requested={} spr_ready={} power={:?}",
            crate::pd::spr_requested(),
            crate::pd::spr_ready(),
            crate::pd::STATUS.try_get()
        ),
        "motion status" => {
            if let Some(state) = crate::accelerometer::MOTION.try_get() {
                log::info!(
                    "Motion {:?}, idle_ms={:?}",
                    state,
                    state.idle_ms(embassy_time::Instant::now().as_millis())
                );
            }
        }
        "" => {}
        _ => log::warn!("RTT commands: pd spr | pd status | motion status | settings status"),
    }
}

#[embassy_executor::task]
pub async fn run(mut input: rtt_target::DownChannel) {
    let mut line = heapless::Vec::<u8, 32>::new();
    let mut discarded = false;
    loop {
        let mut bytes = [0; 64];
        let n = input.read(&mut bytes);
        for &byte in &bytes[..n] {
            if byte == b'\n' {
                if discarded {
                    log::warn!("RTT command too long; discarded");
                } else {
                    execute(&line);
                }
                line.clear();
                discarded = false;
            } else if line.push(byte).is_err() {
                discarded = true;
            }
        }
        Timer::after_millis(20).await;
    }
}
