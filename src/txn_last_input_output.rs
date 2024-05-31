use std::sync::{Arc, RwLock};

use crossbeam::utils::CachePadded;
use revm::primitives::ResultAndState;

use crate::{
    captured_reads::CapturedReads,
    mvhashmap::types::TxnIndex,
    view::{ReadKey, ReadValue},
};

pub struct TxnLastInputOutput {
    inputs: Vec<CachePadded<RwLock<Option<Arc<CapturedReads<ReadKey, ReadValue>>>>>>,
    outputs: Vec<CachePadded<RwLock<Option<Arc<ResultAndState>>>>>,
    writes: Vec<CachePadded<RwLock<Option<Arc<Vec<(ReadKey, ReadValue)>>>>>>,
}

impl TxnLastInputOutput {
    pub fn new(num_txns: TxnIndex) -> Self {
        Self {
            inputs: (0..num_txns)
                .map(|_| CachePadded::new(RwLock::new(None)))
                .collect(),
            outputs: (0..num_txns)
                .map(|_| CachePadded::new(RwLock::new(None)))
                .collect(),
            writes: (0..num_txns)
                .map(|_| CachePadded::new(RwLock::new(None)))
                .collect(),
        }
    }

    pub(crate) fn modified_keys(
        &self,
        txn_idx: TxnIndex,
    ) -> Option<impl Iterator<Item = (ReadKey, ReadValue)>> {
        self.writes[txn_idx as usize]
            .read()
            .unwrap()
            .as_ref()
            .map(|writes| writes.as_ref().clone().into_iter())
    }

    pub(crate) fn record(
        &self,
        txn_idx: TxnIndex,
        input: CapturedReads<ReadKey, ReadValue>,
        output: ResultAndState,
        writes: Vec<(ReadKey, ReadValue)>,
    ) {
        *self.inputs[txn_idx as usize].write().unwrap() = Some(Arc::new(input));
        *self.outputs[txn_idx as usize].write().unwrap() = Some(Arc::new(output));
        *self.writes[txn_idx as usize].write().unwrap() = Some(Arc::new(writes));
    }

    pub(crate) fn read_set(
        &self,
        txn_idx: TxnIndex,
    ) -> Option<Arc<CapturedReads<ReadKey, ReadValue>>> {
        self.inputs[txn_idx as usize].read().unwrap().clone()
    }

    pub(crate) fn take_output(&self, txn_idx: TxnIndex) -> ResultAndState {
        let mut output_guard = self.outputs[txn_idx as usize].write().unwrap();
        let output = output_guard
            .take()
            .expect("[BlockSTM]: Output must be recorded after execution");
        Arc::try_unwrap(output)
            .expect("[BlockSTM]: Output should be uniquely owned after execution")
    }
}
