use crate::SharedI2c;
use bl706_hal::gpio::Input;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};
use embassy_time::Timer;
use pinecil_rs::power::{
    Choice, EPR_ENABLED, EPR_OPERATIONAL_WATTS, PowerStatus, choose, choose_spr, is_spr_request,
};
use uom::si::{electric_current::milliampere, electric_potential::millivolt, power::watt};
use usbpd::{
    protocol_layer::message::data::{
        request::PowerSource,
        sink_capabilities::{FixedSupply, SinkCapabilities, SinkPowerDataObject},
        source_capabilities::{PowerDataObject, SourceCapabilities},
    },
    sink::{
        device_policy_manager::{DevicePolicyManager, DrpDevicePolicyManager, Event, SinkDpm},
        policy_engine::Sink,
    },
};

// Retain the latest contract for independent application/control observers,
// including subscribers that start after negotiation has completed.
pub static STATUS: Watch<CriticalSectionRawMutex, PowerStatus, 3> =
    Watch::new_with(PowerStatus::Waiting);

static SPR_REQUESTED: bl706_hal::portable_atomic::AtomicBool =
    bl706_hal::portable_atomic::AtomicBool::new(!EPR_ENABLED);
static SPR_READY: bl706_hal::portable_atomic::AtomicBool =
    bl706_hal::portable_atomic::AtomicBool::new(false);

pub fn request_spr() {
    SPR_READY.store(false, bl706_hal::portable_atomic::Ordering::Relaxed);
    SPR_REQUESTED.store(true, bl706_hal::portable_atomic::Ordering::Relaxed);
}

pub fn spr_requested() -> bool {
    SPR_REQUESTED.load(bl706_hal::portable_atomic::Ordering::Relaxed)
}

pub fn spr_ready() -> bool {
    SPR_READY.load(bl706_hal::portable_atomic::Ordering::Relaxed)
}

// A diagnostic build can start the PHY after RTT has attached, without a
// source power cycle or a timing-sensitive debugger attach during negotiation.
#[cfg(feature = "pd-manual-start")]
#[unsafe(no_mangle)]
pub static PD_START: bl706_hal::portable_atomic::AtomicU32 =
    bl706_hal::portable_atomic::AtomicU32::new(0);

struct PdTimer;
impl usbpd::timers::Timer for PdTimer {
    async fn after_millis(ms: u64) {
        Timer::after_millis(ms).await;
    }
}

struct Policy {
    pending: Option<Choice>,
    active: Option<(u32, u32)>,
    evaluated_resistance: Option<u32>,
    epr_attempted: bool,
    active_spr: bool,
    exit_requested: bool,
}

fn resistance() -> Option<u32> {
    crate::control::MEASUREMENTS
        .try_get()?
        .heater
        .resistance_mohm
        .ok()
}

impl SinkDpm for Policy {}
impl DrpDevicePolicyManager for Policy {}
impl usbpd::sink::device_policy_manager::EprDevicePolicyManager for Policy {
    async fn epr_mode_entry_failed(
        &mut self,
        reason: usbpd::protocol_layer::message::data::epr_mode::DataEnterFailed,
    ) {
        // Keep the one-attempt guard set through the PE's Soft Reset recovery.
        self.epr_attempted = true;
        log::warn!("PD EPR entry failed: {:?}; retaining SPR fallback", reason);
    }
}
impl DevicePolicyManager for Policy {
    async fn get_event(&mut self, caps: &SourceCapabilities) -> Event {
        if EPR_ENABLED
            && !spr_requested()
            && !self.epr_attempted
            && !caps.is_epr_capabilities()
            && caps.epr_mode_capable()
        {
            self.epr_attempted = true;
            log::debug!(
                "PD entering EPR: operational PDP {} W",
                EPR_OPERATIONAL_WATTS
            );
            return Event::EnterEprMode(usbpd::units::Power::new::<watt>(EPR_OPERATIONAL_WATTS));
        }
        loop {
            if !caps.is_epr_capabilities()
                && self.pending.is_none()
                && self.active_spr
                && !SPR_READY.swap(true, bl706_hal::portable_atomic::Ordering::Relaxed)
            {
                log::info!(
                    "PD SPR ready: EPR inactive, confirmed contract {:?}",
                    self.active
                );
            }
            if spr_requested() && caps.is_epr_capabilities() && !self.exit_requested {
                if self.pending.is_none()
                    && self.active_spr
                    && self
                        .active
                        .is_some_and(|(mv, _)| mv <= pinecil_rs::power::TARGET_MV)
                {
                    // PD 3.2 section 6.4.10: first confirm an SPR PDO through
                    // EPR_Request + PS_RDY, then explicitly exit EPR mode.
                    self.exit_requested = true;
                    self.epr_attempted = true;
                    STATUS.sender().send(PowerStatus::Waiting);
                    log::info!("PD SPR PDO confirmed; sending EPR_Mode(Exit)");
                    return Event::ExitEprMode;
                }
                if self.pending.is_none()
                    && let Some(choice) = choose_spr(caps, resistance())
                {
                    STATUS
                        .sender()
                        .send(PowerStatus::Requesting { mv: choice.mv });
                    log::info!("PD preparing EPR exit: {} mV {} mA", choice.mv, choice.ma);
                    let request = choice.request;
                    SPR_READY.store(false, bl706_hal::portable_atomic::Ordering::Relaxed);
                    self.pending = Some(choice);
                    return Event::RequestPower(request);
                }
            }
            if let Some(r) = resistance()
                && self.evaluated_resistance != Some(r)
            {
                // Evaluate once per new probe result. A Reject must not cause
                // an immediate request loop every time the PE returns Ready.
                self.evaluated_resistance = Some(r);
                let choice = if spr_requested() {
                    choose_spr(caps, Some(r))
                } else {
                    choose(caps, Some(r))
                };
                if let Some(choice) = choice
                    && self.active != Some((choice.mv, choice.ma))
                {
                    log::info!(
                        "PD adapting to cartridge: {} mV {} mA",
                        choice.mv,
                        choice.ma
                    );
                    STATUS
                        .sender()
                        .send(PowerStatus::Requesting { mv: choice.mv });
                    let request = choice.request;
                    SPR_READY.store(false, bl706_hal::portable_atomic::Ordering::Relaxed);
                    self.pending = Some(choice);
                    return Event::RequestPower(request);
                }
            }
            // Cancellation-safe; no bus ownership or borrowed ADC buffer.
            Timer::after_millis(100).await;
        }
    }

    async fn request(&mut self, caps: &SourceCapabilities) -> PowerSource {
        SPR_READY.store(false, bl706_hal::portable_atomic::Ordering::Relaxed);
        log::debug!(
            "PD {} capabilities: {} PDOs",
            if caps.is_epr_capabilities() {
                "EPR"
            } else {
                "SPR"
            },
            caps.pdos().len()
        );
        for (i, pdo) in caps.pdos().iter().enumerate() {
            if let PowerDataObject::FixedSupply(f) = pdo {
                log::debug!(
                    "PD source PDO{}: {} mV {} mA",
                    i + 1,
                    f.voltage().get::<millivolt>(),
                    f.max_current().get::<milliampere>()
                );
            }
        }
        let load = resistance();
        if !caps.is_epr_capabilities() {
            self.exit_requested = false;
        }
        let choice = if spr_requested() {
            choose_spr(caps, load)
        } else {
            choose(caps, load)
        };
        let Some(choice) = choice else {
            log::error!("PD invalid source capabilities; no request sent");
            STATUS.sender().send(PowerStatus::Error);
            return core::future::pending().await;
        };
        self.evaluated_resistance = load;
        log::info!("PD requesting {} mV {} mA", choice.mv, choice.ma);
        STATUS
            .sender()
            .send(PowerStatus::Requesting { mv: choice.mv });
        let request = choice.request;
        self.pending = Some(choice);
        request
    }

    async fn transition_power(&mut self, accepted: &PowerSource) {
        if let Some(c) = self.pending.take() {
            self.active_spr = is_spr_request(accepted);
            self.active = Some((c.mv, c.ma));
            // usbpd calls this only after Accept followed by PS_RDY.
            log::info!("PD contract ready: {} mV {} mA (PS_RDY)", c.mv, c.ma);
            STATUS
                .sender()
                .send(PowerStatus::Contract { mv: c.mv, ma: c.ma });
        }
    }

    async fn hard_reset(&mut self) {
        SPR_READY.store(false, bl706_hal::portable_atomic::Ordering::Relaxed);
        self.pending = None;
        self.active = None;
        self.evaluated_resistance = None;
        self.epr_attempted = false;
        self.active_spr = false;
        self.exit_requested = false;
        STATUS.sender().send(PowerStatus::Waiting);
        log::info!("PD hard reset: waiting for default power");
    }

    fn sink_capabilities(&self) -> SinkCapabilities {
        let mut pdos = heapless::Vec::new();
        pdos.push(SinkPowerDataObject::FixedSupply(FixedSupply::new_vsafe5v(
            300,
        )))
        .unwrap();
        pdos.push(SinkPowerDataObject::FixedSupply(FixedSupply::new(400, 300)))
            .unwrap();
        SinkCapabilities::new(pdos)
    }
}

#[embassy_executor::task]
pub async fn run(i2c: SharedI2c, irq: Input<16>) {
    #[cfg(feature = "pd-diagnostics")]
    {
        use core::future::{Future, poll_fn};
        let mut future = core::pin::pin!(run_inner(i2c, irq));
        poll_fn(|cx| {
            let start = embassy_time::Instant::now();
            let result = future.as_mut().poll(cx);
            crate::pd_trace::profile(9, start);
            result
        })
        .await;
    }
    #[cfg(not(feature = "pd-diagnostics"))]
    run_inner(i2c, irq).await;
}

async fn run_inner(i2c: SharedI2c, mut irq: Input<16>) {
    STATUS.sender().send(PowerStatus::Waiting);
    #[cfg(feature = "pd-manual-start")]
    {
        log::info!("PD diagnostic: waiting for debugger to write PD_START=1");
        while PD_START.load(bl706_hal::portable_atomic::Ordering::Relaxed) == 0 {
            Timer::after_millis(10).await;
        }
    }
    log::info!("PD initializing FUSB302B at 0x22, INT_N GPIO16");
    #[cfg(feature = "pd-manual-start")]
    let i2c = {
        let mut i2c = i2c;
        if PD_START.load(bl706_hal::portable_atomic::Ordering::Relaxed) == 2 {
            crate::pd_trace::cancellation_test(&mut i2c).await;
            PD_START.store(0, bl706_hal::portable_atomic::Ordering::Relaxed);
            log::info!("PD diagnostic: cancellation test done; waiting for PD_START=1");
            while PD_START.load(bl706_hal::portable_atomic::Ordering::Relaxed) == 0 {
                Timer::after_millis(10).await;
            }
        }
        i2c
    };
    #[cfg(feature = "pd-diagnostics")]
    let i2c = crate::pd_trace::TracedI2c(i2c);
    // Pinecil V2 leaves the FUSB302 VBUS sense pin disconnected (IronOS:
    // NEEDS_VBUS_PROBE=0). The powered board must not wait on STATUS0.VBUSOK.
    // External detector `()` follows that board policy; it is not a voltage ADC.
    let mut waiting_for_cc = false;
    let driver = loop {
        match fusb302b::Fusb302b::init_with_vbus_and_interrupt(
            i2c.clone(),
            (),
            fusb302b::GpioInterrupt(&mut irq),
        )
        .await
        {
            Ok(driver) => break driver,
            Err(fusb302b::FusbError::NoCcDetected) => {
                // VBUS can power the MCU before the source's CC is detectable,
                // e.g. while inserting an inline meter. This is not a fatal
                // bus error. Retry with the output interlocked, no board reset.
                if !waiting_for_cc {
                    log::info!("PD waiting for source CC; retrying initialization");
                    waiting_for_cc = true;
                }
                Timer::after_millis(250).await;
            }
            Err(e) => {
                log::error!("PD init failed: {:?}", e);
                STATUS.sender().send(PowerStatus::Error);
                return;
            }
        }
    };
    if EPR_ENABLED {
        log::info!("PD controller ready; SPR 20 V then EPR 28 V preferred");
    } else {
        log::info!("PD controller ready; SPR only, voltage constrained by cartridge current");
    }
    let mut sink = Sink::<_, PdTimer, _>::new(
        driver,
        Policy {
            pending: None,
            active: None,
            evaluated_resistance: None,
            epr_attempted: false,
            active_spr: false,
            exit_requested: false,
        },
    );
    let result = sink.run().await;
    match result {
        Ok(_) => log::error!("PD engine stopped"),
        Err(usbpd::sink::policy_engine::Error::Protocol(_)) => log::error!("PD protocol error"),
        Err(_) => log::error!("PD source unresponsive"),
    }
    STATUS.sender().send(PowerStatus::Error);
}
