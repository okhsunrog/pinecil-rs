//! Standalone example observer for the shared GATT implementation.
use pinecil_rs::ble::{self, State};

pub async fn run(transport: bl706_ble::NativeTransport) -> ! {
    ble::run(transport, observe).await
}

fn observe(state: State) {
    trace!("BLE: {:?}", state);
    #[cfg(feature = "ble-diagnostics")]
    match state {
        State::Advertising => {
            crate::diagnostics::record(10, 0);
            crate::diagnostics::anchor();
        }
        State::Connected => crate::diagnostics::record(11, 0),
        State::Disconnected => crate::diagnostics::record(12, 0),
    }
}
