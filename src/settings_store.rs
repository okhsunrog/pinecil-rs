//! Versioned settings records on sequential-storage, independent of the HAL.
use crate::settings::Settings;
use embedded_storage_async::nor_flash::NorFlash;
use sequential_storage::{
    cache::{
        Cache, key_pointers::ArrayKeyPointers, page_pointers::ArrayPagePointers,
        page_states::ArrayPageStates,
    },
    map::{MapConfig, MapStorage, SerializationError, Value},
};
use serde::{Deserialize, Serialize};

const KEY: u8 = 1;
#[derive(Serialize, Deserialize)]
struct Record {
    schema: u16,
    settings: Settings,
}
// Keep postcard's default heapless 0.7 feature disabled: it assumes CAS
// atomics unavailable on this IMFC target. Only slice serialization is needed.
impl<'a> Value<'a> for Record {
    fn serialize_into(&self, buffer: &mut [u8]) -> Result<usize, SerializationError> {
        postcard::to_slice(self, buffer)
            .map(|s| s.len())
            .map_err(|_| SerializationError::BufferTooSmall)
    }
    fn deserialize_from(buffer: &'a [u8]) -> Result<(Self, usize), SerializationError> {
        postcard::from_bytes(buffer)
            .map(|v| (v, buffer.len()))
            .map_err(|_| SerializationError::InvalidFormat)
    }
}

#[derive(Debug)]
pub enum Error<E> {
    Flash(sequential_storage::Error<E>),
    Schema(u16),
    InvalidSettings,
}

// Four pages and one settings key: bounded RAM caches avoid rescanning the
// entire log on every edit. Caches are recreated after reset/recovery.
type SettingsCache = Cache<ArrayPageStates<4>, ArrayPagePointers<4>, ArrayKeyPointers<u8, 1>, u8>;
pub struct Store<F: NorFlash> {
    map: MapStorage<u8, F, SettingsCache>,
}
impl<F: NorFlash> Store<F> {
    pub fn new(flash: F) -> Self {
        let end = flash.capacity() as u32;
        assert_eq!(
            flash.capacity(),
            4 * F::ERASE_SIZE,
            "settings requires four erase pages"
        );
        Self {
            map: MapStorage::new(
                flash,
                MapConfig::new(0..end),
                Cache::new(
                    ArrayPageStates::new(),
                    ArrayPagePointers::new(),
                    ArrayKeyPointers::new(),
                ),
            ),
        }
    }
    pub async fn load(&mut self) -> Result<Option<Settings>, Error<F::Error>> {
        let mut buffer = [0u8; 64];
        let Some(record) = self
            .map
            .fetch_item::<Record>(&mut buffer, &KEY)
            .await
            .map_err(Error::Flash)?
        else {
            return Ok(None);
        };
        if record.schema != 1 {
            return Err(Error::Schema(record.schema));
        }
        if !record.settings.valid() {
            return Err(Error::InvalidSettings);
        }
        Ok(Some(record.settings))
    }
    pub async fn save(&mut self, settings: Settings) -> Result<(), Error<F::Error>> {
        if !settings.valid() {
            return Err(Error::InvalidSettings);
        }
        let mut buffer = [0u8; 64];
        self.map
            .store_item(
                &mut buffer,
                &KEY,
                &Record {
                    schema: 1,
                    settings,
                },
            )
            .await
            .map_err(Error::Flash)
    }
    pub fn flash_mut(&mut self) -> &mut F {
        self.map.flash()
    }
    pub fn into_flash(self) -> F {
        self.map.destroy().0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embassy_futures::block_on;
    use embedded_storage_async::nor_flash::{
        ErrorType, NorFlashError, NorFlashErrorKind, ReadNorFlash,
    };
    use std::vec::Vec;

    #[derive(Debug)]
    struct PowerCut;
    impl NorFlashError for PowerCut {
        fn kind(&self) -> NorFlashErrorKind {
            NorFlashErrorKind::Other
        }
    }
    #[derive(Clone)]
    struct Memory {
        data: Vec<u8>,
        budget: Option<usize>,
        erases: usize,
    }
    impl Memory {
        fn new() -> Self {
            Self {
                data: std::vec![255;1024],
                budget: None,
                erases: 0,
            }
        }
        fn step(&mut self) -> Result<(), PowerCut> {
            if let Some(left) = &mut self.budget {
                if *left == 0 {
                    return Err(PowerCut);
                }
                *left -= 1;
            }
            Ok(())
        }
    }
    impl ErrorType for Memory {
        type Error = PowerCut;
    }
    impl ReadNorFlash for Memory {
        const READ_SIZE: usize = 1;
        fn capacity(&self) -> usize {
            self.data.len()
        }
        async fn read(&mut self, offset: u32, data: &mut [u8]) -> Result<(), PowerCut> {
            data.copy_from_slice(&self.data[offset as usize..offset as usize + data.len()]);
            Ok(())
        }
    }
    impl NorFlash for Memory {
        const WRITE_SIZE: usize = 1;
        const ERASE_SIZE: usize = 256;
        async fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), PowerCut> {
            for (i, &byte) in data.iter().enumerate() {
                self.step()?;
                let old = &mut self.data[offset as usize + i];
                assert_eq!(*old & byte, byte);
                *old = byte;
            }
            Ok(())
        }
        async fn erase(&mut self, from: u32, to: u32) -> Result<(), PowerCut> {
            assert!(from.is_multiple_of(256) && to.is_multiple_of(256));
            self.erases += 1;
            for i in from..to {
                self.step()?;
                self.data[i as usize] = 255;
            }
            Ok(())
        }
    }
    #[test]
    fn erased_storage_and_whole_snapshot_survive_reopening_and_gc() {
        block_on(async {
            let mut store = Store::new(Memory::new());
            assert_eq!(store.load().await.unwrap(), None);
            for i in 0..300 {
                let settings = Settings {
                    setpoint_c: 150 + i % 200,
                    brightness: 1 + (i % 4) as u8,
                    ..Settings::default()
                };
                store.save(settings).await.unwrap();
                store = Store::new(store.into_flash());
                assert_eq!(store.load().await.unwrap(), Some(settings));
            }
            assert!(store.into_flash().erases > 4);
        });
    }
    #[test]
    fn every_interrupted_record_recovers_an_entire_old_or_new_snapshot() {
        block_on(async {
            let old = Settings::default();
            let new = Settings {
                setpoint_c: 230,
                brightness: 4,
                rotation: crate::settings::RotationMode::Left,
            };
            let mut base = Store::new(Memory::new());
            base.save(old).await.unwrap();
            let base = base.into_flash();
            for cut in 0..64 {
                let mut flash = base.clone();
                flash.budget = Some(cut);
                let mut store = Store::new(flash);
                let _ = store.save(new).await;
                let mut flash = store.into_flash();
                flash.budget = None;
                let mut recovered = Store::new(flash);
                let actual = recovered.load().await.unwrap().unwrap();
                assert!(actual == old || actual == new, "cut={cut}");
                recovered.save(new).await.unwrap();
                assert_eq!(recovered.load().await.unwrap(), Some(new));
            }
        });
    }
    #[test]
    fn interrupted_page_migration_recovers_and_can_be_written_again() {
        block_on(async {
            let mut store = Store::new(Memory::new());
            let mut old = Settings::default();
            store.save(old).await.unwrap();
            // Find a save that reclaims a page containing older records.
            let base = loop {
                let before = store.into_flash();
                let before_erases = before.erases;
                store = Store::new(before.clone());
                let new = Settings {
                    setpoint_c: if old.setpoint_c == 150 { 155 } else { 150 },
                    ..old
                };
                store.save(new).await.unwrap();
                let after = store.into_flash();
                let migrated = after.erases > before_erases && before_erases >= 4;
                store = Store::new(after);
                if migrated {
                    break before;
                }
                old = new;
            };
            let new = Settings {
                brightness: 4,
                rotation: crate::settings::RotationMode::Left,
                ..old
            };
            for cut in 0..600 {
                let mut flash = base.clone();
                flash.budget = Some(cut);
                let mut interrupted = Store::new(flash);
                let _ = interrupted.save(new).await;
                let mut flash = interrupted.into_flash();
                flash.budget = None;
                let mut recovered = Store::new(flash);
                let value = recovered.load().await.unwrap().unwrap();
                assert!(value == old || value == new, "cut={cut}: {value:?}");
                recovered.save(new).await.unwrap();
                assert_eq!(recovered.load().await.unwrap(), Some(new));
            }
        });
    }
    #[test]
    fn future_schema_and_invalid_values_are_rejected_without_overwriting() {
        block_on(async {
            let mut store = Store::new(Memory::new());
            let mut buffer = [0; 64];
            store
                .map
                .store_item(
                    &mut buffer,
                    &KEY,
                    &Record {
                        schema: 2,
                        settings: Settings::default(),
                    },
                )
                .await
                .unwrap();
            assert!(matches!(store.load().await, Err(Error::Schema(2))));
            store
                .map
                .store_item(
                    &mut buffer,
                    &KEY,
                    &Record {
                        schema: 1,
                        settings: Settings {
                            brightness: 0,
                            ..Settings::default()
                        },
                    },
                )
                .await
                .unwrap();
            assert!(matches!(store.load().await, Err(Error::InvalidSettings)));
        });
    }
}
