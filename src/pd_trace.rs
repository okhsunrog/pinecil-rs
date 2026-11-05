//! Bounded RAM I2C trace and RTT diagnostics for PD investigation.
use crate::SharedI2c;
use bl706_hal::portable_atomic::{AtomicU32, Ordering::Relaxed};
use core::cell::RefCell;
use embedded_hal_async::i2c::{ErrorType, I2c, Operation};

static CHANNEL: critical_section::Mutex<RefCell<Option<rtt_target::UpChannel>>> =
    critical_section::Mutex::new(RefCell::new(None));

pub fn init_channel(channel: rtt_target::UpChannel) {
    #[cfg(feature = "ble")]
    for record in &PD_TRACE {
        // This buffer lives in a dedicated NOLOAD region above RF scratch.
        // Initialize it before BLE calibration or any PD task can run.
        record.start_us.store(0, Relaxed);
        record.end_us.store(0, Relaxed);
        record.metadata.store(0, Relaxed);
        for word in &record.data {
            word.store(0, Relaxed);
        }
    }
    critical_section::with(|cs| *CHANNEL.borrow_ref_mut(cs) = Some(channel));
}

/// Set to one only on a stable 5 V source to benchmark the old verbose log.
#[unsafe(no_mangle)]
pub static PD_VERBOSE_HEARTBEAT: AtomicU32 = AtomicU32::new(0);

/// CPU duration record: address zero, register identifying the measured stage.
pub fn profile(stage: u8, start: embassy_time::Instant) {
    let record = Record::new();
    record.start_us.store(start.as_micros() as u32, Relaxed);
    record
        .end_us
        .store(embassy_time::Instant::now().as_micros() as u32, Relaxed);
    record.metadata.store(u32::from(stage) << 8, Relaxed);
    record.stream();
}

#[repr(C)]
pub struct Record {
    start_us: AtomicU32,
    end_us: AtomicU32,
    metadata: AtomicU32,
    data: [AtomicU32; 8],
}
impl Record {
    const fn new() -> Self {
        Self {
            start_us: AtomicU32::new(0),
            end_us: AtomicU32::new(0),
            metadata: AtomicU32::new(0),
            data: [const { AtomicU32::new(0) }; 8],
        }
    }
    fn data(&self, bytes: &[u8]) {
        for (word, chunk) in self.data.iter().zip(bytes.chunks(4)) {
            let mut b = [0; 4];
            b[..chunk.len()].copy_from_slice(chunk);
            word.store(u32::from_le_bytes(b), Relaxed);
        }
    }
    fn stream(&self) {
        // Fixed binary records avoid formatting packet dumps on the PD task's
        // response path. NoBlockSkip drops a whole record if the host is absent.
        let mut bytes = [0; 44];
        let words = [
            self.start_us.load(Relaxed),
            self.end_us.load(Relaxed),
            self.metadata.load(Relaxed),
            self.data[0].load(Relaxed),
            self.data[1].load(Relaxed),
            self.data[2].load(Relaxed),
            self.data[3].load(Relaxed),
            self.data[4].load(Relaxed),
            self.data[5].load(Relaxed),
            self.data[6].load(Relaxed),
            self.data[7].load(Relaxed),
        ];
        for (out, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
            out.copy_from_slice(&word.to_le_bytes());
        }
        critical_section::with(|cs| {
            if let Some(channel) = &mut *CHANNEL.borrow_ref_mut(cs) {
                channel.write(&bytes);
            }
        });
    }
}
#[unsafe(no_mangle)]
#[cfg_attr(feature = "ble", unsafe(link_section = ".pd_trace"))]
pub static PD_TRACE: [Record; 256] = [const { Record::new() }; 256];
#[unsafe(no_mangle)]
pub static PD_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static OVERFLOW: Record = Record::new();

struct Guard {
    record: &'static Record,
    finished: bool,
}
impl Guard {
    fn new(address: u8, register: u8, read: bool, len: usize, data: &[u8]) -> Self {
        let index = PD_TRACE_COUNT.fetch_add(1, Relaxed) as usize;
        let record = PD_TRACE.get(index).unwrap_or(&OVERFLOW);
        record.end_us.store(0, Relaxed);
        record.metadata.store(
            u32::from(address)
                | (u32::from(register) << 8)
                | (u32::from(read) << 16)
                | ((len as u32) << 17),
            Relaxed,
        );
        record.data(data);
        record
            .start_us
            .store(embassy_time::Instant::now().as_micros() as u32, Relaxed);
        Self {
            record,
            finished: false,
        }
    }
    fn finish(&mut self, ok: bool) {
        if !ok {
            self.record.metadata.fetch_or(1 << 30, Relaxed);
        }
        self.record
            .end_us
            .store(embassy_time::Instant::now().as_micros() as u32, Relaxed);
        self.finished = true;
        if !ok || (self.record.metadata.load(Relaxed) >> 8) & 255 == 0x43 {
            self.record.stream();
        }
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if !self.finished {
            self.record.metadata.fetch_or(1 << 31, Relaxed);
            self.record
                .end_us
                .store(embassy_time::Instant::now().as_micros() as u32, Relaxed);
            self.record.stream();
            let meta = self.record.metadata.load(Relaxed);
            log::warn!(
                "PD I2C cancelled reg={:02x} read={} len={} started={}",
                (meta >> 8) & 255,
                meta & (1 << 16) != 0,
                (meta >> 17) & 8191,
                self.record.start_us.load(Relaxed),
            );
        }
    }
}

pub struct TracedI2c(pub SharedI2c);
impl ErrorType for TracedI2c {
    type Error = <SharedI2c as ErrorType>::Error;
}

#[cfg(feature = "pd-manual-start")]
pub async fn cancellation_test(i2c: &mut SharedI2c) {
    use embassy_futures::select::{Either, select};
    use embassy_time::Timer;
    let mut cancellations = 0;
    let mut failures = 0;
    for delay_us in [0, 50, 100, 200, 400, 800, 1600] {
        let mut cancelled = 0;
        let mut failed = 0;
        for _ in 0..20 {
            let mut id = [0];
            match select(
                i2c.write_read(0x22, &[1], &mut id),
                Timer::after_micros(delay_us),
            )
            .await
            {
                Either::First(result) => {
                    if let Err(e) = result {
                        log::warn!("I2C pre-cancel read: {:?}", e);
                    }
                }
                Either::Second(()) => cancelled += 1,
            }
            let mut id = [0];
            match i2c.write_read(0x22, &[1], &mut id).await {
                Ok(()) if id[0] == 0x81 => {}
                result => {
                    failed += 1;
                    log::warn!(
                        "I2C after cancellation delay_us={}: {:?} id={:02x}",
                        delay_us,
                        result,
                        id[0]
                    );
                }
            }
            Timer::after_millis(2).await;
        }
        cancellations += cancelled;
        failures += failed;
        log::info!(
            "I2C cancellation delay_us={} cancelled={}/20 followup_failures={}",
            delay_us,
            cancelled,
            failed
        );
    }
    log::info!(
        "I2C cancellation test complete: cancelled={} followup_failures={}",
        cancellations,
        failures
    );
}
impl I2c for TracedI2c {
    async fn transaction(
        &mut self,
        address: u8,
        operations: &mut [Operation<'_>],
    ) -> Result<(), Self::Error> {
        let (register, read, len, data) = match &*operations {
            [Operation::Write(bytes)] => (
                bytes.first().copied().unwrap_or(0),
                false,
                bytes.len().saturating_sub(1),
                bytes.get(1..).unwrap_or(&[]),
            ),
            [Operation::Write(sub), Operation::Read(bytes)] => (
                sub.first().copied().unwrap_or(0),
                true,
                bytes.len(),
                &[][..],
            ),
            _ => (0, false, 0, &[][..]),
        };
        let mut guard = Guard::new(address, register, read, len, data);
        let result = self.0.transaction(address, operations).await;
        if result.is_ok()
            && let [Operation::Write(_), Operation::Read(bytes)] = operations
        {
            guard.record.data(bytes);
        }
        guard.finish(result.is_ok());
        if let Err(error) = &result {
            log::warn!(
                "PD I2C reg={:02x} read={} len={}: {:?}",
                register,
                read,
                len,
                error
            );
        }
        result
    }
}
