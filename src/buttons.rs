use bl706_hal::gpio::Input;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Instant, with_timeout};
use pinecil_rs::app::ButtonEvents;
use pinecil_rs::ui::{Button, ButtonEvent};

pub static EVENTS: ButtonEvents<CriticalSectionRawMutex> = ButtonEvents::new();
const DEBOUNCE: Duration = Duration::from_millis(25);
const HOLD: Duration = Duration::from_millis(700);

/// Wait for a level to remain unchanged for the debounce interval. Edges wake
/// the task through GPIO IRQs; the timeout restarts only after an actual edge.
async fn stable<const N: u8>(pin: &mut Input<N>, high: bool) {
    loop {
        if high {
            pin.wait_for_high().await;
        } else {
            pin.wait_for_low().await;
        }
        if with_timeout(DEBOUNCE, pin.wait_for_any_edge())
            .await
            .is_err()
            && pin.is_high() == high
        {
            return;
        }
    }
}

async fn run<const N: u8>(mut pin: Input<N>, button: Button) {
    loop {
        stable(&mut pin, true).await;
        let start = Instant::now();
        log::debug!("button {:?} pressed gpio={}", button, N);
        let event = if with_timeout(HOLD, stable(&mut pin, false)).await.is_err() {
            let event = ButtonEvent::Long(button);
            EVENTS.send(event).await;
            log::debug!("button {:?} event={:?}", button, event);
            stable(&mut pin, false).await;
            log::debug!(
                "button {:?} released after {} ms",
                button,
                start.elapsed().as_millis()
            );
            continue;
        } else {
            ButtonEvent::Short(button)
        };
        EVENTS.send(event).await;
        log::debug!(
            "button {:?} event={:?} held={} ms",
            button,
            event,
            start.elapsed().as_millis()
        );
    }
}

#[embassy_executor::task]
pub async fn plus(pin: Input<25>) {
    run(pin, Button::Plus).await;
}

#[embassy_executor::task]
pub async fn minus(pin: Input<28>) {
    run(pin, Button::Minus).await;
}
