use std::hash::Hash;
use std::sync::Arc;

use derivative::Derivative;
use revm::primitives::HashMap;

use crate::mvhashmap::types::{ReadError, ReadOutput, TxnIndex, Version};
use crate::mvhashmap::versioned_data::VersionedData;

#[derive(Derivative)]
#[derivative(Clone(bound = ""), Debug(bound = ""), PartialEq(bound = ""))]
pub(crate) struct DataRead<T> {
    pub version: Version,
    #[derivative(PartialEq = "ignore", Debug = "ignore")]
    pub value: Arc<T>,
}

impl<T> DataRead<T> {
    // TODO deleteable?
    pub fn new(version: Version, value: Arc<T>) -> Self {
        Self {
            version,
            value: value.clone(), // tbh idk why we clone here but aptos does
        }
    }
}

// TODO add the transaction generic?
#[derive(Debug)]
pub(crate) struct CapturedReads<K, V> {
    // TODO should include value in the version?
    // and potentially create DataRead type
    reads: HashMap<K, DataRead<V>>,
    /// If there is a speculative failure (e.g. delta application failure, or an
    /// observed inconsistency), the transaction output is irrelevant (must be
    /// discarded and transaction re-executed). We have a global flag, as which
    /// read observed the inconsistency is irrelevant (moreover, typically,
    /// an error is returned to the VM to wrap up the ongoing execution).
    speculative_failure: bool,
    /// Set if the invarint on CapturedReads intended use is violated. Leads to an alert
    /// and sequential execution fallback.
    incorrect_use: bool,
}

impl<K, V> Default for CapturedReads<K, V> {
    fn default() -> Self {
        Self {
            reads: HashMap::new(),
            speculative_failure: false,
            incorrect_use: false,
        }
    }
}

impl<K: Eq + Hash + Clone, V: Clone> CapturedReads<K, V> {
    pub(crate) fn capture_read(&mut self, key: K, read: DataRead<V>) {
        self.reads.insert(key, read);
    }

    pub(crate) fn get(&self, state_key: &K) -> Option<DataRead<V>> {
        self.reads.get(state_key).and_then(|r| Some(r.clone()))
    }

    pub(crate) fn is_incorrect_use(&self) -> bool {
        self.incorrect_use
    }

    pub(crate) fn validate_reads(
        &self,
        data_map: &VersionedData<K, V>,
        idx_to_validate: TxnIndex,
    ) -> bool {
        if self.speculative_failure {
            return false;
        }

        use ReadError::*;
        use ReadOutput::*;
        self.reads
            .iter()
            .all(|(k, r)| match data_map.fetch_data(k, idx_to_validate) {
                // TODO check the following line, might be wrong
                // it should be sufficient to check the versions for equality since we don't have
                // ordering over the types of reads (full, metadata, exist). aptos does this dataread
                // stuff since their dataread has ordering.
                Ok(Versioned(version, _v)) => r.version == version,
                Err(Dependency(_)) | Err(Uninitialized) => false,
            })
    }

    pub(crate) fn mark_failure(&mut self) {
        self.speculative_failure = true;
    }

    pub(crate) fn mark_incorrect_use(&mut self) {
        self.incorrect_use = true;
    }
}
