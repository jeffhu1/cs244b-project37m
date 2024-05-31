use crate::mvhashmap::versioned_data::VersionedData;
use crate::view::{ReadKey, ReadValue};

pub struct MVHashMap {
    data: VersionedData<ReadKey, ReadValue>,
}

impl MVHashMap {
    pub fn new() -> MVHashMap {
        MVHashMap {
            data: VersionedData::new(),
        }
    }

    pub fn data(&self) -> &VersionedData<ReadKey, ReadValue> {
        &self.data
    }
}
