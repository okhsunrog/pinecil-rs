use crate::SharedI2c;
use bl706_hal::gpio::Input;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};
use embassy_time::Timer;
use pinecil_rs::power::{Choice, EPR_OPERATIONAL_WATTS, PowerStatus, choose};
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
    epr_attempted: bool,
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
        if !self.epr_attempted && !caps.is_epr_capabilities() && caps.epr_mode_capable() {
            self.epr_attempted = true;
            log::debug!(
                "PD entering EPR: operational PDP {} W",
                EPR_OPERATIONAL_WATTS
            );
            return Event::EnterEprMode(usbpd::units::Power::new::<watt>(EPR_OPERATIONAL_WATTS));
        }
        core::future::pending().await
    }

    async fn request(&mut self, caps: &SourceCapabilities) -> PowerSource {
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
        let Some(choice) = choose(caps) else {
            log::error!("PD invalid source capabilities; no request sent");
            STATUS.sender().send(PowerStatus::Error);
            return core::future::pending().await;
        };
        log::debug!("PD requesting {} mV {} mA", choice.mv, choice.ma);
        let request = choice.request;
        self.pending = Some(choice);
        request
    }

    async fn transition_power(&mut self, _: &PowerSource) {
        if let Some(c) = self.pending.take() {
            // usbpd calls this only after Accept followed by PS_RDY.
            log::info!("PD contract ready: {} mV {} mA (PS_RDY)", c.mv, c.ma);
            STATUS
                .sender()
                .send(PowerStatus::Contract { mv: c.mv, ma: c.ma });
        }
    }

    async fn hard_reset(&mut self) {
        self.pending = None;
        self.epr_attempted = false;
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

async fn run_inner(i2c: SharedI2c, irq: Input<16>) {
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
    let driver = match fusb302b::Fusb302b::init_with_vbus_and_interrupt(
        i2c,
        (),
        fusb302b::GpioInterrupt(irq),
    )
    .await
    {
        Ok(driver) => driver,
        Err(e) => {
            log::error!("PD init failed: {:?}", e);
            STATUS.sender().send(PowerStatus::Error);
            return;
        }
    };
    log::info!("PD controller ready; SPR 20 V then EPR 28 V preferred");
    let mut sink = Sink::<_, PdTimer, _>::new(
        driver,
        Policy {
            pending: None,
            epr_attempted: false,
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
