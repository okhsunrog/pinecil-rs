//! Shared-bus SC7A20 acquisition. Motion policy stays independent of I2C.
use crate::SharedI2c;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};
use embassy_time::{Duration, Instant, Ticker, Timer, with_timeout};
use pinecil_rs::motion::{MotionState, SAMPLE_STALE_MS, Status, Tracker};
use sc7a20_dd::{Address, Config, DataRate, FifoConfig, Sc7a20Async};

pub static MOTION: Watch<CriticalSectionRawMutex, MotionState, 1> =
    Watch::new_with(MotionState::WAITING);

#[embassy_executor::task]
pub async fn run(i2c: SharedI2c) {
    let mut accel = Sc7a20Async::new(i2c, Address::Low);
    let mut tracker = Tracker::default();
    let sender = MOTION.sender();
    Timer::after_millis(150).await;
    loop {
        let initialized = with_timeout(Duration::from_millis(250), async {
            accel.check_device_id().await?;
            // Reinitialization must not retain a previous FIFO experiment.
            accel.configure_fifo(FifoConfig::default()).await?;
            // Gravity must remain in XYZ even if an earlier image enabled HPF.
            accel
                .ll
                .ctrl2()
                .modify_async(|r| r.set_filtered_data(false))
                .await?;
            accel
                .configure(Config {
                    data_rate: DataRate::Hz25,
                    ..Config::default()
                })
                .await
        })
        .await;
        if !matches!(initialized, Ok(Ok(()))) {
            if MOTION
                .try_get()
                .is_none_or(|state| state.status != Status::Unavailable)
            {
                log::warn!(
                    "SC7A20 unavailable: {:?}; keeping display orientation",
                    initialized
                );
            }
            sender.send(tracker.unavailable());
            Timer::after_secs(2).await;
            continue;
        }
        log::info!("SC7A20 ready: 0x18, HR +/-2g, 25Hz; motion does not enable heating");
        Timer::after_millis(120).await;
        let mut ticker = Ticker::every(Duration::from_millis(40));
        let mut last_sample = Instant::now();
        let mut next_publish = last_sample;
        let mut orientation = MOTION.try_get().unwrap().orientation;
        loop {
            ticker.next().await;
            let sample = with_timeout(Duration::from_millis(100), async {
                if accel.status().await?.xyz_ready() {
                    accel.read_raw().await.map(Some)
                } else {
                    Ok(None)
                }
            })
            .await;
            match sample {
                Ok(Ok(Some(raw))) => {
                    last_sample = Instant::now();
                    let state = tracker.observe([raw.x, raw.y, raw.z], last_sample.as_millis());
                    if state.orientation != orientation {
                        log::info!(
                            "SC7A20 orientation {:?}, raw={:?}",
                            state.orientation,
                            state.raw
                        );
                        orientation = state.orientation;
                        next_publish = last_sample;
                    }
                    // Coalesce raw samples to 4 Hz; publish rotations immediately.
                    if last_sample >= next_publish {
                        sender.send(state);
                        next_publish = last_sample + Duration::from_millis(250);
                    }
                }
                Ok(Ok(None)) if last_sample.elapsed().as_millis() <= SAMPLE_STALE_MS => {}
                error => {
                    log::warn!("SC7A20 read failed/stale: {:?}; retrying", error);
                    break;
                }
            }
        }
        sender.send(tracker.unavailable());
        Timer::after_secs(2).await;
    }
}
