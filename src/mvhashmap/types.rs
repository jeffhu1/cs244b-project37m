use std::sync::Arc;

pub type TxnIndex = u32;
pub type Incarnation = u32;

/// Custom error type representing storage version. Result<Index, StorageVersion>
/// then represents either index of some type (i.e. TxnIndex, Version), or a
/// version corresponding to the storage (pre-block) state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageVersion;

// TODO: Find better representations for this, a similar one for TxnIndex.
pub type Version = Result<(TxnIndex, Incarnation), StorageVersion>;

pub(crate) enum Flag {
    Done,
    Estimate,
}

/// Represents errors that can occur while reading from `VersionedData`.
#[derive(Debug)]
pub enum ReadError {
    /// Indicates a dependency error, where an entry is marked as an estimate.
    /// This typically means that the data is not yet finalized and should not be relied upon.
    Dependency(TxnIndex),

    /// Indicates that the requested data is uninitialized.
    /// This error is returned when trying to access data that does not exist.
    Uninitialized,
}
/// Represents the possible outcomes of a read operation on `VersionedData`.
#[derive(Debug)]
pub enum ReadOutput<V> {
    /// This outcome indicates the data has been successfully read. It includes an optional pair of a transaction index
    /// and an incarnation number, along with a shared reference to an `Account` object. Using `Arc<Account>` allows
    /// the account data to be safely shared across multiple threads without duplicating the data, which helps in reducing memory usage.
    Versioned(Version, Arc<V>),
}

// In order to store base vales at the lowest index, i.e. at index 0, without conflicting
// with actual transaction index 0, the following struct wraps the index and internally
// increments it by 1.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Debug)]
pub(crate) struct ShiftedTxnIndex {
    idx: TxnIndex,
}

impl ShiftedTxnIndex {
    pub fn new(real_idx: TxnIndex) -> Self {
        Self { idx: real_idx + 1 }
    }

    pub(crate) fn idx(&self) -> Result<TxnIndex, StorageVersion> {
        if self.idx > 0 {
            Ok(self.idx - 1)
        } else {
            Err(StorageVersion)
        }
    }

    pub(crate) fn zero_idx() -> Self {
        Self { idx: 0 }
    }
}
