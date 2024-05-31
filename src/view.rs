use revm::{
    precompile::B256,
    primitives::{AccountInfo, Address, U256},
    Database, DatabaseRef,
};
use std::cell::RefCell;

use crate::{
    captured_reads::{CapturedReads, DataRead},
    errors::PanicError,
    mvhashmap::{
        mvhashmap::MVHashMap,
        types::{ReadError, ReadOutput, TxnIndex},
    },
    scheduler::{DependencyResult, DependencyStatus, Scheduler, TWaitForDependency},
};

#[derive(Debug, Eq, PartialEq, Hash, Clone)]
pub enum ReadKey {
    Basic(Address),
    Storage(Address, U256),
}

#[derive(Debug, Clone)]
pub enum ReadValue {
    Basic(Option<AccountInfo>),
    Storage(U256),
}

/// A struct which describes the result of the read from the proxy. The client
/// can interpret these types to further resolve the reads.
#[derive(Debug)]
pub(crate) enum ReadResult<V> {
    Value(V),
    Uninitialized,
    // Must halt the execution of the calling transaction. This might be because
    // there was an inconsistency in observed speculative state, or dependency
    // waiting indicated that the parallel execution had been halted. The String
    // parameter provides more context (error description / message).
    HaltSpeculativeExecution(String),
}

pub(crate) struct ParallelState<'a> {
    pub(crate) versioned_map: &'a MVHashMap,
    scheduler: &'a Scheduler,
    captured_reads: RefCell<CapturedReads<ReadKey, ReadValue>>,
}

// txn_idx is estimated to have a r/w dependency on dep_idx.
// Returns after the dependency has been resolved, the returned indicator is true if
// it is safe to continue, and false if the execution has been halted.
fn wait_for_dependency(
    wait_for: &dyn TWaitForDependency,
    txn_idx: TxnIndex,
    dep_idx: TxnIndex,
) -> Result<bool, PanicError> {
    match wait_for.wait_for_dependency(txn_idx, dep_idx)? {
        DependencyResult::Dependency(dep_condition) => {
            // Wait on a condition variable corresponding to the encountered
            // read dependency. Once the dep_idx finishes re-execution, scheduler
            // will mark the dependency as resolved, and then the txn_idx will be
            // scheduled for re-execution, which will re-awaken cvar here.
            // A deadlock is not possible due to these condition variables:
            // suppose all threads are waiting on read dependency, and consider
            // one with lowest txn_idx. It observed a dependency, so some thread
            // aborted dep_idx. If that abort returned execution task, by
            // minimality (lower transactions aren't waiting), that thread would
            // finish execution unblock txn_idx, contradiction. Otherwise,
            // execution_idx in scheduler was lower at a time when at least the
            // thread that aborted dep_idx was alive, and again, since lower txns
            // than txn_idx are not blocked, so the execution of dep_idx will
            // eventually finish and lead to unblocking txn_idx, contradiction.
            let (lock, cvar) = &*dep_condition;
            let mut dep_resolved = lock.lock();
            while matches!(*dep_resolved, DependencyStatus::Unresolved) {
                dep_resolved = cvar.wait(dep_resolved).unwrap();
            }
            // dep resolved status is either resolved or execution halted.
            Ok(matches!(*dep_resolved, DependencyStatus::Resolved))
        }
        DependencyResult::ExecutionHalted => Ok(false),
        DependencyResult::Resolved => Ok(true),
    }
}

impl<'a> ParallelState<'a> {
    pub fn new(shared_map: &'a MVHashMap, shared_scheduler: &'a Scheduler) -> Self {
        Self {
            versioned_map: shared_map,
            scheduler: shared_scheduler,
            captured_reads: RefCell::new(CapturedReads::default()),
        }
    }

    fn set_base_value(&self, key: ReadKey, value: ReadValue) {
        self.versioned_map.data().set_base_value(key, value)
    }

    // missing fields: target_kind, layout, patch_base_value
    fn read_cached_data(&self, txn_idx: TxnIndex, key: &ReadKey) -> ReadResult<ReadValue> {
        use ReadError::*;
        use ReadOutput::*;

        if let Some(data) = self.captured_reads.borrow().get(key) {
            // TODO is this right?
            return ReadResult::Value((*data.value).clone());
        }

        loop {
            match self.versioned_map.data().fetch_data(key, txn_idx) {
                Ok(Versioned(version, value)) => {
                    let data_read = DataRead::new(version, value);
                    self.captured_reads
                        .borrow_mut()
                        .capture_read(key.clone(), data_read.clone());
                    return ReadResult::Value((*data_read.value).clone());
                }
                Err(Uninitialized) => {
                    // The underlying assumption here for not recording anything about the read is
                    // that the caller is expected to initialize the contents and serve the reads
                    // solely via the 'fetch_read' interface. Thus, the later, successful read,
                    // will make the needed recordings.
                    return ReadResult::Uninitialized;
                }
                Err(Dependency(dep_idx)) => {
                    match wait_for_dependency(self.scheduler, txn_idx, dep_idx) {
                        Err(e) => {
                            println!("Error {:?} in wait for dependency", e);
                            self.captured_reads.borrow_mut().mark_incorrect_use();
                            return ReadResult::HaltSpeculativeExecution(format!(
                                "Error {:?} in wait for dependency",
                                e
                            ));
                        }
                        Ok(false) => {
                            self.captured_reads.borrow_mut().mark_failure();
                            return ReadResult::HaltSpeculativeExecution(
                                "Interrupted as block execution was halted".to_string(),
                            );
                        }
                        Ok(true) => {
                            // dependency resolved
                        }
                    }
                }
            }
        }
    }
}

pub struct InstrumentedDB<'a, ExtDB> {
    base_view: ExtDB,
    latest_view: ParallelState<'a>,
    txn_idx: TxnIndex,
}

impl<'a, ExtDB> InstrumentedDB<'a, ExtDB> {
    pub fn new(base_view: ExtDB, latest_view: ParallelState<'a>, txn_idx: TxnIndex) -> Self {
        Self {
            base_view,
            latest_view,
            txn_idx,
        }
    }

    pub fn take_parallel_reads(&self) -> CapturedReads<ReadKey, ReadValue> {
        self.latest_view.captured_reads.take()
    }
}

impl<'a, ExtDB: DatabaseRef> Database for InstrumentedDB<'a, ExtDB> {
    type Error = ExtDB::Error;

    fn basic(
        &mut self,
        address: Address,
    ) -> Result<Option<revm::primitives::AccountInfo>, Self::Error> {
        #[cfg(debug_assertions)]
        println!("basic {:?} {:?}", address, std::thread::current().id());
        let key = ReadKey::Basic(address);

        let mut ret = self.latest_view.read_cached_data(self.txn_idx, &key);
        if matches!(ret, ReadResult::Uninitialized) {
            let from_storage = match self.base_view.basic_ref(address) {
                Ok(data) => data,
                Err(e) => return Err(e),
            };

            self.latest_view
                .set_base_value(key.clone(), ReadValue::Basic(from_storage));
            ret = self.latest_view.read_cached_data(self.txn_idx, &key);
        }

        match ret {
            ReadResult::HaltSpeculativeExecution(_msg) => {
                unimplemented!("TODO");
            }
            ReadResult::Uninitialized => {
                unreachable!("base value must already be recorded in the MV data structure")
            }
            ReadResult::Value(ReadValue::Storage(_)) => {
                unreachable!("basic call should not pull a storage value from mvhashmap");
            }
            ReadResult::Value(ReadValue::Basic(res)) => return Ok(res),
        }
    }

    fn code_by_hash(
        &mut self,
        _code_hash: B256,
    ) -> Result<revm::primitives::Bytecode, Self::Error> {
        // TODO: Check if this is actually used by anyone
        println!("code_by_hash");
        panic!("should not be called");
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        #[cfg(debug_assertions)]
        println!("storage {:?}", std::thread::current().id());
        let key = ReadKey::Storage(address, index);

        let mut ret = self.latest_view.read_cached_data(self.txn_idx, &key);
        if matches!(ret, ReadResult::Uninitialized) {
            let from_storage = match self.base_view.storage_ref(address, index) {
                Ok(data) => data,
                Err(e) => return Err(e),
            };

            self.latest_view
                .set_base_value(key.clone(), ReadValue::Storage(from_storage));
            ret = self.latest_view.read_cached_data(self.txn_idx, &key);
        }

        match ret {
            ReadResult::HaltSpeculativeExecution(_msg) => {
                unimplemented!("TODO");
            }
            ReadResult::Uninitialized => {
                unreachable!("base value must already be recorded in the MV data structure")
            }
            ReadResult::Value(ReadValue::Basic(_)) => {
                unreachable!("storage call should not pull a basic value from mvhashmap");
            }
            ReadResult::Value(ReadValue::Storage(res)) => return Ok(res),
        }
    }

    // NOTE: We don't include block_hash_ref in the read set because
    // the hash of the 256 most recent blocks cannot be modified by the
    // execution result of a given transaction within the current block
    fn block_hash(&mut self, number: U256) -> Result<B256, Self::Error> {
        println!("block_hash");
        self.base_view.block_hash_ref(number)
    }
}
