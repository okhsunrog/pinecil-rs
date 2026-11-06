//! Shared bring-up GATT service. Writes change only a counter, never the heater.
use core::future::Future;
use embassy_futures::{
    join::join,
    select::{Either, select},
};
use embassy_time::Timer;
use trouble_host::prelude::*;

#[gatt_server]
struct Server {
    test: TestService,
}
#[gatt_service(uuid = "b1706000-0000-4000-8000-000000000001")]
struct TestService {
    #[characteristic(
        uuid = "b1706001-0000-4000-8000-000000000001",
        read,
        write,
        notify,
        value = 0
    )]
    counter: u32,
}

/// Handshake for flash maintenance. Advertising is stopped and acknowledged
/// by HCI before granting a permit. Active connections deny the request.
pub struct FlashGate {
    request: embassy_sync::signal::Signal<
        embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
        (),
    >,
    ready: embassy_sync::signal::Signal<
        embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
        bool,
    >,
    resume: embassy_sync::signal::Signal<
        embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
        (),
    >,
}
impl Default for FlashGate {
    fn default() -> Self {
        Self::new()
    }
}
impl FlashGate {
    pub const fn new() -> Self {
        Self {
            request: embassy_sync::signal::Signal::new(),
            ready: embassy_sync::signal::Signal::new(),
            resume: embassy_sync::signal::Signal::new(),
        }
    }
    /// Single storage owner only. Do not cancel after requesting a permit.
    pub async fn pause(&self) -> bool {
        self.request.signal(());
        self.ready.wait().await
    }
    pub fn resume(&self) {
        self.resume.signal(());
    }
}

async fn requested(gate: Option<&FlashGate>) {
    match gate {
        Some(g) => g.request.wait().await,
        None => core::future::pending().await,
    }
}

#[derive(Clone, Copy, Debug)]
pub enum State {
    Advertising,
    Connected,
    Disconnected,
}

/// Run the existing counter service alongside the application. State hooks let
/// the standalone diagnostic example record events without coupling it to UI.
pub async fn run(transport: bl706_ble::NativeTransport, on_state: fn(State)) -> ! {
    run_with_flash_gate(transport, on_state, None).await
}

pub async fn run_with_flash_gate(
    transport: bl706_ble::NativeTransport,
    on_state: fn(State),
    gate: Option<&FlashGate>,
) -> ! {
    let controller = bt_hci::controller::ExternalController::<_, 4>::new(transport);
    let mut resources: HostResources<DefaultPacketPool, 1, 2> = HostResources::new();
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(Address::random([0x06, 0x07, 0x0b, 0, 0, 0xc2]))
        .build();
    let mut runner = stack.runner();
    let mut peripheral = stack.peripheral();
    let server = Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: "Pinecil Rust",
        appearance: &appearance::power_device::GENERIC_POWER_DEVICE,
    }))
    .unwrap();
    join(
        async {
            runner.run().await.expect("Trouble runner failed");
        },
        async {
            loop {
                let mut data = [0; 31];
                let len = AdStructure::encode_slice(
                    &[
                        AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
                        AdStructure::CompleteLocalName(b"Pinecil Rust"),
                    ],
                    &mut data,
                )
                .unwrap();
                let advertiser = peripheral
                    .advertise(
                        &Default::default(),
                        Advertisement::ConnectableScannableUndirected {
                            adv_data: &data[..len],
                            scan_data: &[],
                        },
                    )
                    .await
                    .unwrap();
                on_state(State::Advertising);
                log::info!("BLE advertising Pinecil Rust; heater OFF");
                // Retain the Advertiser future across the pause. Dropping it
                // would ask Trouble to stop advertising a second time; this
                // controller rejects that duplicate HCI command.
                let accepted = {
                    let mut accept = core::pin::pin!(advertiser.accept());
                    loop {
                        match select(accept.as_mut(), requested(gate)).await {
                            Either::First(result) => break result,
                            Either::Second(()) => {
                                let stopped = stack
                                    .command(bt_hci::cmd::le::LeSetAdvEnable::new(false))
                                    .await;
                                // A connection may have won the race with stop.
                                let completed = core::future::poll_fn(|cx| {
                                    core::task::Poll::Ready(accept.as_mut().poll(cx))
                                })
                                .await;
                                if let core::task::Poll::Ready(result) = completed {
                                    gate.unwrap().ready.signal(false);
                                    break result;
                                }
                                stopped.expect("stop advertising");
                                let gate = gate.unwrap();
                                gate.ready.signal(true);
                                gate.resume.wait().await;
                                stack
                                    .command(bt_hci::cmd::le::LeSetAdvEnable::new(true))
                                    .await
                                    .expect("resume advertising");
                            }
                        }
                    }
                };
                let conn = accepted.unwrap().with_attribute_server(&server).unwrap();
                on_state(State::Connected);
                log::info!("BLE connected");
                select(
                    async {
                        loop {
                            requested(gate).await;
                            gate.unwrap().ready.signal(false);
                        }
                    },
                    select(
                        async {
                            loop {
                                match conn.next().await {
                                    GattConnectionEvent::Disconnected { reason } => {
                                        on_state(State::Disconnected);
                                        log::info!("BLE disconnected {:?}", reason);
                                        break;
                                    }
                                    GattConnectionEvent::Gatt { event } => {
                                        if let GattEvent::Write(ref write) = event {
                                            write.with_data(|offset, data| {
                                                log::info!(
                                                    "BLE: GATT write offset={} data={:02x?}",
                                                    offset,
                                                    data
                                                );
                                            });
                                        }
                                        event.accept().unwrap().send().await;
                                    }
                                    _ => {}
                                }
                            }
                        },
                        async {
                            loop {
                                Timer::after_secs(1).await;
                                let value = conn.get(&server.test.counter).unwrap().wrapping_add(1);
                                if server
                                    .test
                                    .counter
                                    .notify(&conn, &value, true)
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        },
                    ),
                )
                .await;
            }
        },
    )
    .await;
    panic!("BLE tasks unexpectedly exited")
}
