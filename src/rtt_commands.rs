//! Operator commands over RTT; no UART or extra GPIO wiring is required.
use embassy_time::Timer;

fn execute(line: &[u8]) {
    let Ok(command) = core::str::from_utf8(line) else {
        log::warn!("RTT command must be UTF-8");
        return;
    };
    match command.trim() {
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
        "" => {}
        _ => log::warn!("RTT commands: pd spr | pd status"),
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
