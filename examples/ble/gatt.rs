//! Test-only GATT service. Writable value is a counter, never a heater command.
use embassy_futures::{join::join, select::select};
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

pub async fn run(transport: bl706_ble::NativeTransport) -> ! {
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
                trace!("BLE: advertising Pinecil Rust; heater OFF");
                let conn = advertiser
                    .accept()
                    .await
                    .unwrap()
                    .with_attribute_server(&server)
                    .unwrap();
                trace!("BLE: connected");
                select(
                    async {
                        loop {
                            match conn.next().await {
                                GattConnectionEvent::Disconnected { reason } => {
                                    trace!("BLE: disconnected {:?}", reason);
                                    break;
                                }
                                GattConnectionEvent::Gatt { event } => {
                                    if let GattEvent::Write(ref write) = event {
                                        write.with_data(|offset, data| {
                                            trace!(
                                                "BLE: GATT write offset={} data={:02x?}",
                                                offset, data
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
                )
                .await;
            }
        },
    )
    .await;
    panic!("BLE tasks unexpectedly exited")
}
