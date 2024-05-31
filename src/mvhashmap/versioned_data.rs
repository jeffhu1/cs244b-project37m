/**
 * `VersionedData` is designed to handle versioned data entries associated with specific blockchain addresses.
 * It supports simultaneous read and write operations on these entries, maintaining consistency of data throughout transactions.
 * Essentially, it acts as a caching layer for transactional writes to the blockchain
 *
 * Internally, `VersionedData` utilizes a `DashMap` for concurrency, mapping each `Address` to a `BTreeMap`.
 * The `BTreeMap` then maps transaction indices (`TxnIndex`) to their corresponding data entries (`Entry`),
 * allowing for efficient retrieval of an account's state at any given transaction index.
 */
use std::{
    collections::btree_map::{self, BTreeMap},
    hash::Hash,
    sync::Arc,
};

use crossbeam::utils::CachePadded;
use dashmap::DashMap;

use crate::mvhashmap::types::{
    Flag, Incarnation, ReadError, ReadOutput, ShiftedTxnIndex, TxnIndex,
};

/// Represents a single entry in the versioned data map.
///
/// Each `Entry` contains the state of an account at a specific point in its transaction history.
/// This allows the `VersionedData` structure to track and retrieve the account's state across different transactions.
struct Entry<V> {
    // Actual account data.
    account: V,
    // Incarnation number of the transaction that wrote this entry.
    incarnation: Incarnation,
    // Used to mark the entry as an "estimate".
    flag: Flag,
}

impl<V> Entry<V> {
    fn new(incarnation: Incarnation, account: V) -> Self {
        Self {
            account,
            incarnation,
            flag: Flag::Done,
        }
    }
    fn mark_estimate(&mut self) {
        self.flag = Flag::Estimate;
    }
}

/// A versioned value internally is represented as a BTreeMap from indices of
/// transactions that update the given access path & the corresponding entries.
struct VersionedValue<V> {
    versioned_map: BTreeMap<ShiftedTxnIndex, CachePadded<Entry<V>>>,
}

impl<V> Default for VersionedValue<V> {
    fn default() -> Self {
        Self {
            versioned_map: BTreeMap::new(),
        }
    }
}

impl<V: Clone> VersionedValue<V> {
    /// Note: our custom read function
    fn read(&self, txn_idx: TxnIndex) -> Result<ReadOutput<V>, ReadError> {
        let mut iter = self
            .versioned_map
            .range(ShiftedTxnIndex::zero_idx()..ShiftedTxnIndex::new(txn_idx));

        while let Some((idx, entry)) = iter.next_back() {
            match entry.flag {
                Flag::Estimate => {
                    return Err(ReadError::Dependency(
                        idx.idx().expect("May not depend on storage version"),
                    ));
                }
                Flag::Done => {
                    return Ok(ReadOutput::Versioned(
                        idx.idx().map(|idx| (idx, entry.incarnation)),
                        Arc::new(entry.account.clone()),
                    ));
                }
            }
        }
        // Returns an uninitialized error if no entry is found.
        Err(ReadError::Uninitialized)
    }
}

/// `VersionedData` manages versioned entries of data mapped to specific addresses.
/// It allows for concurrent reads and writes to these entries, ensuring data consistency
/// across transactions. The data is organized in a way that each address can have multiple
/// entries, each associated with a different transaction index, allowing for the retrieval
/// of the state of an account at any point in its history.
pub struct VersionedData<K, V> {
    data: DashMap<K, VersionedValue<V>>,
}

impl<K: Eq + Hash + Clone, V: Clone> VersionedData<K, V> {
    pub fn new() -> Self {
        Self {
            data: DashMap::new(),
        }
    }

    /// Writes a new `Entry` into the `versioned_map` for a specified `address`.
    /// This function takes the address, transaction index, incarnation, and account to create a new entry.
    /// It ensures that each entry is unique per transaction index for an address.
    ///
    /// # Arguments
    /// * `address` - The blockchain address for which to write the entry.
    /// * `txn_idx` - The transaction index associated with this entry.
    /// * `incarnation` - The incarnation number of the transaction that wrote this entry.
    /// * `account` - The account data to be stored.
    ///
    /// # Panics
    /// Panics if an entry with the same transaction index but a higher or equal incarnation number already exists.
    pub fn write(&self, address: K, txn_idx: TxnIndex, incarnation: Incarnation, account: V) {
        // Accesses or creates a new `VersionedValue` for the specified address, then inserts the new entry.
        let mut v = self.data.entry(address).or_default();

        // Check if there's a previous entry for the same transaction index.
        if let Some(prev_entry) = v.versioned_map.get(&ShiftedTxnIndex::new(txn_idx)) {
            // Assert that the previous entry's incarnation number is lower than the new one.
            assert!(
                prev_entry.incarnation < incarnation,
                "Previous entry has a higher incarnation number"
            );
        }

        v.versioned_map.insert(
            ShiftedTxnIndex::new(txn_idx),
            CachePadded::new(Entry::new(incarnation, account)),
        );
    }

    pub fn fetch_data(&self, key: &K, txn_idx: TxnIndex) -> Result<ReadOutput<V>, ReadError> {
        self.data
            .get(key)
            .map(|v| v.read(txn_idx))
            .unwrap_or(Err(ReadError::Uninitialized))
    }

    /// This is used to set a base value - in other words, when
    /// the value doesn't exist in the mvhashmap and was never written
    /// by a previous transaction
    pub fn set_base_value(&self, key: K, value: V) {
        let mut v = self.data.entry(key).or_default();
        // For base value, incarnation is irrelevant, and is always set to 0.

        use btree_map::Entry::*;
        match v.versioned_map.entry(ShiftedTxnIndex::zero_idx()) {
            Vacant(v) => {
                // TODO: there might be concurrency errors here, since
                // aptos checks for an atomic byte length while that
                // doesn't occur here. But probably not, it seems like
                // the total_base_value_size is primarily (exclusively?)
                // used for delayed fields.
                v.insert(CachePadded::new(Entry::new(0, value)));
            }
            Occupied(mut _o) => {
                // TODO: Do nothing, in aptos this works with exchanging value
                // with schema backed value and some other logic but
                // should be unnecessary here. This is because we always
                // only get typed data, never raw bytes.
            }
        }
    }

    /// Marks an `Entry` as an estimate for a given `address` and `txn_idx`.
    /// This function is used to update the flag of an existing entry to `Flag::Estimate`.
    pub fn mark_estimate(&self, address: &K, txn_idx: TxnIndex) {
        let mut entry_map = self.data.get_mut(address).expect("Address must exist");
        entry_map
            .versioned_map
            .get_mut(&ShiftedTxnIndex::new(txn_idx))
            .expect("Entry by the txn must exist to mark estimate")
            .mark_estimate();
    }

    /// Removes an `Entry` from the `versioned_map` for a given `address` and `txn_idx`.
    /// This function is used to delete an existing entry from the map.
    pub fn remove(&self, address: &K, txn_idx: TxnIndex) {
        let mut entry_map = self.data.get_mut(address).expect("Address must exist");
        entry_map
            .versioned_map
            .remove(&ShiftedTxnIndex::new(txn_idx))
            .expect("Entry by the txn must exist to be deleted");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvhashmap::types::ReadOutput;
    use revm::primitives::{keccak256, Account, AccountStatus, Address, Storage, U256};
    use revm::primitives::{AccountInfo, Bytecode, B256};

    /// Const function for making an address by concatenating the bytes from two given numbers.
    ///
    /// Note that 32 + 128 = 160 = 20 bytes (the length of an address). This function is used
    /// as a convenience for specifying the addresses of the various precompiles.
    /// This is copied from the revm crate.
    pub fn u64_to_address(x: u64) -> Address {
        let x = x.to_be_bytes();
        Address::new([
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, x[0], x[1], x[2], x[3], x[4], x[5], x[6], x[7],
        ])
    }

    // Helper function to create a test account
    fn create_test_account(balance: U256, nonce: u64, code: Bytecode) -> Account {
        let code_hash = B256::from_slice(&keccak256(&code.original_bytes()).as_ref());
        Account {
            info: AccountInfo::new(balance, nonce, code_hash, code),
            storage: Storage::new(),
            status: AccountStatus::Loaded,
        }
    }

    #[test]
    fn set_base_value_simple() {
        let versioned_data = VersionedData::new();
        let address = u64_to_address(1);
        let account = create_test_account(U256::from(100), 0, Bytecode::new());
        versioned_data.set_base_value(address.clone(), account.clone());

        let ans = versioned_data
            .fetch_data(&address, 0)
            .expect("should return a value");

        match ans {
            ReadOutput::Versioned(v, a) => {
                assert!(matches!(v, Err(_))); // should be error bc base read
                assert_eq!(
                    Arc::try_unwrap(a).expect("Arc should only have one owner here"),
                    account
                );
            }
            _ => {
                panic!("should output a versioned type");
            }
        }
    }

    #[test]
    fn base_value_complex() {
        // check that base value works even after setting another value
        // afterwards - that is, pull the latest value even if base
        // value was set previously. also check interactions where
        // base value is set after?
    }
}
