use bl706_hal::gpio::Input;
use embassy_futures::select::select3;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Instant, Timer};
use pinecil_rs::{app::ButtonEvents, button_gestures::Gestures};

pub static EVENTS: ButtonEvents<CriticalSectionRawMutex> = ButtonEvents::new();

/// Recognize chords before individual holds. Idle inputs sleep on GPIO IRQs;
/// a short timer services debounce and hold deadlines while either is active.
#[embassy_executor::task]
pub async fn run(mut plus: Input<25>, mut minus: Input<28>) {
    let mut gestures = Gestures::default();
    loop {
        if let Some(event) =
            gestures.update(Instant::now().as_millis(), plus.is_high(), minus.is_high())
        {
            EVENTS.send(event).await;
        }
        let delay_ms = if gestures.idle() { 60_000 } else { 10 };
        select3(
            plus.wait_for_any_edge(),
            minus.wait_for_any_edge(),
            Timer::after_millis(delay_ms),
        )
        .await;
    }
}
