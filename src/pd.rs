use crate::SharedI2c;
use bl706_hal::gpio::Input;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::Timer;
use pinecil_rs::power::{Choice, PowerStatus, choose};
use uom::si::{electric_current::milliampere, electric_potential::millivolt};
use usbpd::{
    protocol_layer::message::data::{
        request::PowerSource,
        sink_capabilities::{FixedSupply, SinkCapabilities, SinkPowerDataObject},
        source_capabilities::{PowerDataObject, SourceCapabilities},
    },
    sink::{
        device_policy_manager::{DevicePolicyManager, DrpDevicePolicyManager, SinkDpm},
        policy_engine::Sink,
    },
};

pub static STATUS: Signal<CriticalSectionRawMutex, PowerStatus> = Signal::new();

struct PdTimer;
impl usbpd::timers::Timer for PdTimer {
    async fn after_millis(ms: u64) {
        Timer::after_millis(ms).await;
    }
}

struct Policy {
    pending: Option<Choice>,
}

impl SinkDpm for Policy {}
impl DrpDevicePolicyManager for Policy {}
impl usbpd::sink::device_policy_manager::EprDevicePolicyManager for Policy {}
impl DevicePolicyManager for Policy {
    async fn request(&mut self, caps: &SourceCapabilities) -> PowerSource {
        for (i, pdo) in caps.pdos().iter().enumerate() {
            if let PowerDataObject::FixedSupply(f) = pdo {
                log::info!(
                    "PD source PDO{}: {} mV {} mA",
                    i + 1,
                    f.voltage().get::<millivolt>(),
                    f.max_current().get::<milliampere>()
                );
            }
        }
        let Some(choice) = choose(caps) else {
            log::error!("PD invalid source capabilities; no request sent");
            STATUS.signal(PowerStatus::Error);
            return core::future::pending().await;
        };
        log::info!("PD requesting {} mV {} mA", choice.mv, choice.ma);
        STATUS.signal(PowerStatus::Requesting { mv: choice.mv });
        let request = choice.request;
        self.pending = Some(choice);
        request
    }

    async fn transition_power(&mut self, _: &PowerSource) {
        if let Some(c) = self.pending.take() {
            // usbpd calls this only after Accept followed by PS_RDY.
            log::info!("PD contract ready: {} mV {} mA (PS_RDY)", c.mv, c.ma);
            STATUS.signal(PowerStatus::Contract { mv: c.mv, ma: c.ma });
        }
    }

    async fn hard_reset(&mut self) {
        self.pending = None;
        STATUS.signal(PowerStatus::Waiting);
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
    STATUS.signal(PowerStatus::Waiting);
    log::info!("PD initializing FUSB302B at 0x22, INT_N GPIO16");
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
            STATUS.signal(PowerStatus::Error);
            return;
        }
    };
    log::info!("PD controller ready; starting SPR sink, fixed 20 V preferred");
    let mut sink = Sink::<_, PdTimer, _>::new(driver, Policy { pending: None });
    let result = sink.run().await;
    match result {
        Ok(_) => log::error!("PD engine stopped"),
        Err(usbpd::sink::policy_engine::Error::Protocol(_)) => log::error!("PD protocol error"),
        Err(_) => log::error!("PD source unresponsive"),
    }
    STATUS.signal(PowerStatus::Error);
}
