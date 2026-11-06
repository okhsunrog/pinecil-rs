//! One flash owner: coalesce edits, persist snapshots, never persist heat enable.
use bl706_hal::flash::XipFlash;
use bl706_hal::portable_atomic::{AtomicBool, Ordering};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};
use embassy_time::{Duration, Instant, Timer};
use pinecil_rs::{
    app::{SettingsCommands, StateReceiver},
    heater,
    settings::Settings,
    settings_store::Store,
};

pub static COMMANDS: SettingsCommands<CriticalSectionRawMutex> = SettingsCommands::new();
#[cfg(feature = "storage-diagnostics")]
pub static STRESS: AtomicBool = AtomicBool::new(false);
pub static BUSY: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "ble")]
pub static BLE_GATE: pinecil_rs::ble::FlashGate = pinecil_rs::ble::FlashGate::new();
#[derive(Clone, Copy, Debug)]
pub enum Phase {
    Clean,
    Pending,
    Saving,
    Error,
    Unavailable,
}
#[derive(Clone, Copy, Debug)]
pub struct Status {
    pub phase: Phase,
    pub saved: Option<Settings>,
    pub metrics: bl706_hal::flash::XipMetrics,
}
pub static STATUS: Watch<CriticalSectionRawMutex, Status, 1> = Watch::new();

pub fn open(token: bl706_hal::flash::FlashToken) -> (Option<Store<XipFlash>>, Settings) {
    mod layout {
        include!(concat!(env!("OUT_DIR"), "/flash_layout.rs"));
    }
    // SAFETY: linker and image builder exclude this range; no tasks, radio,
    // DMA or heater are running. ROM transaction code is linked in copied RAM.
    let flash =
        unsafe { XipFlash::new_mx25l8006(token, layout::SETTINGS_START..layout::SETTINGS_END) };
    let mut status = Status {
        phase: Phase::Clean,
        saved: None,
        metrics: Default::default(),
    };
    let result = match flash {
        Ok(mut flash) => {
            flash.set_erase_allowed(true); // boot recovery before radio/watchdog
            let mut store = Store::new(flash);
            match embassy_futures::block_on(store.load()) {
                Ok(saved) => {
                    store.flash_mut().set_erase_allowed(false);
                    status.metrics = store.flash_mut().metrics();
                    status.saved = saved;
                    Some(store)
                }
                Err(error) => {
                    log::error!("Settings load {:?}; defaults, writes disabled", error);
                    None
                }
            }
        }
        Err(error) => {
            log::error!("Settings flash {:?}; defaults", error);
            None
        }
    };
    if result.is_none() {
        status.phase = Phase::Unavailable;
    }
    let initial = status.saved.unwrap_or_default();
    log::info!("Settings boot {:?}; {:?}", initial, status.phase);
    STATUS.sender().send(status);
    (result, initial)
}

#[embassy_executor::task]
pub async fn run(
    mut store: Option<Store<XipFlash>>,
    mut snapshots: StateReceiver<'static, CriticalSectionRawMutex>,
) {
    let Some(store) = store.as_mut() else {
        core::future::pending::<()>().await;
        unreachable!()
    };
    let sender = STATUS.sender();
    let mut status = STATUS.try_get().unwrap();
    let mut current = snapshots.get().await;
    // Empty flash is left empty until the user actually changes a setting.
    let mut observed = current.ui.settings;
    let mut due = None;
    loop {
        if let Some(latest) = snapshots.try_changed() {
            current = latest;
        }
        if current.ui.settings != observed {
            observed = current.ui.settings;
            due = if status.saved == Some(observed) {
                None
            } else {
                Some(Instant::now() + Duration::from_millis(1500))
            };
            status.phase = if due.is_some() {
                Phase::Pending
            } else {
                Phase::Clean
            };
            sender.send(status);
        }
        #[cfg(feature = "storage-diagnostics")]
        if STRESS.load(Ordering::Acquire) {
            due = Some(Instant::now());
        }
        let off = !current.ui.heating_requested
            && current.measurements.heater.state == heater::State::Off;
        // Long sector erase would miss EPR KeepAlive. Retain the dirty snapshot
        // and retry after an explicit SPR transition rather than change policy.
        let spr = matches!(
            current.power,
            pinecil_rs::power::PowerStatus::Contract { mv: 1..=20_000, .. }
        );
        if due.is_some_and(|t| Instant::now() >= t) && off && spr && crate::pd::spr_ready() {
            // Block a new heat request while awaiting the radio permit, then
            // recheck the actual output before invoking any flash operation.
            BUSY.store(true, Ordering::Release);
            #[cfg(feature = "ble")]
            let radio_ready = BLE_GATE.pause().await;
            #[cfg(not(feature = "ble"))]
            let radio_ready = true;
            let still_off = crate::APP_STATE
                .try_get()
                .is_some_and(|s| !s.ui.heating_requested)
                && crate::control::MEASUREMENTS
                    .try_get()
                    .is_some_and(|s| s.heater.state == heater::State::Off);
            let still_spr = crate::pd::spr_ready()
                && matches!(
                    crate::pd::STATUS.try_get(),
                    Some(pinecil_rs::power::PowerStatus::Contract { mv: 1..=20_000, .. })
                );
            if radio_ready && still_off && still_spr {
                store.flash_mut().set_erase_allowed(true);
                status.phase = Phase::Saving;
                sender.send(status);
                #[cfg(feature = "storage-diagnostics")]
                if STRESS.swap(false, Ordering::AcqRel) {
                    log::info!("Settings GC stress start; original={:?}", observed);
                    for i in 0..1800u32 {
                        let test = Settings {
                            brightness: 1 + (i % 4) as u8,
                            ..observed
                        };
                        if let Err(error) = store.save(test).await {
                            log::error!("Settings GC stress failed at {}: {:?}", i, error);
                            break;
                        }
                    }
                    log::info!(
                        "Settings GC stress complete {:?}",
                        store.flash_mut().metrics()
                    );
                }
                let result = store.save(observed).await;
                store.flash_mut().set_erase_allowed(false);
                status.metrics = store.flash_mut().metrics();
                match result {
                    Ok(()) => {
                        status.saved = Some(observed);
                        status.phase = Phase::Clean;
                        due = None;
                        log::info!("Settings saved {:?}; {:?}", observed, status.metrics);
                    }
                    Err(error) => {
                        status.phase = Phase::Error;
                        due = Some(Instant::now() + Duration::from_secs(5));
                        log::error!("Settings save {:?}; retry pending", error);
                    }
                }
                sender.send(status);
            }
            #[cfg(feature = "ble")]
            if radio_ready {
                BLE_GATE.resume();
            }
            BUSY.store(false, Ordering::Release);
        }
        Timer::after_millis(100).await;
    }
}
