use crate::errors::{code_invariant_error, PanicError};
use crate::explicit_sync_wrapper::ExplicitSyncWrapper;
use crate::mvhashmap::{
    mvhashmap::MVHashMap,
    types::{Incarnation, TxnIndex},
};
use crate::scheduler::{DependencyStatus, ExecutionTaskType, Scheduler, SchedulerTask, Wave};
use crate::txn_last_input_output::TxnLastInputOutput;
use crate::view::ParallelState;
use crate::view::{InstrumentedDB, ReadKey, ReadValue};
use ethers_core::types::{Block, Transaction};
use rayon::ThreadPool;
use revm::primitives::{EVMError, InvalidTransaction, ResultAndState};
use revm::{
    primitives::{Address, HashMap, TransactTo, U256},
    DatabaseRef, Evm,
};
use std::fmt;
use std::{marker::PhantomData, sync::Arc};

macro_rules! local_fill {
    ($left:expr, $right:expr, $fun:expr) => {
        if let Some(right) = $right {
            $left = $fun(right.0)
        }
    };
    ($left:expr, $right:expr) => {
        if let Some(right) = $right {
            $left = Address::from(right.as_fixed_bytes())
        }
    };
}
// Custom error type
#[derive(Debug)]
struct ExecutionError(String);

impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Execution error: {}", self.0)
    }
}

impl std::error::Error for ExecutionError {}

// TODO missing a transaction commit hook
#[derive(Debug)]
pub struct BlockExecutor<DB> {
    concurrency_level: usize, // TODO: this doesn't match the aptos config, double check
    executor_thread_pool: Arc<ThreadPool>,
    phantom: PhantomData<DB>,
}

impl<DB> BlockExecutor<DB>
where
    DB: DatabaseRef + Sync + Send,
    <DB as DatabaseRef>::Error: std::fmt::Debug, // Add this line
{
    pub fn new(concurrency_level: usize, executor_thread_pool: Arc<ThreadPool>) -> Self {
        Self {
            concurrency_level,
            executor_thread_pool,
            phantom: PhantomData,
        }
    }

    fn execute<'a>(
        idx_to_execute: TxnIndex,
        incarnation: Incarnation,
        block: &'a Block<Transaction>,
        last_input_output: &'a TxnLastInputOutput,
        versioned_cache: &'a MVHashMap,
        executor: Evm<'a, (), InstrumentedDB<'a, &'a DB>>,
        base_view: &'a DB,
        latest_view: ParallelState<'a>,
        chain_id: u64,
    ) -> Result<(bool, Evm<'a, (), InstrumentedDB<'a, &'a DB>>), PanicError> {
        #[cfg(debug_assertions)]
        println!(
            "Executing transaction {} with incarnation {} on thread {:?}",
            idx_to_execute,
            incarnation,
            std::thread::current().id()
        );
        let tx = &block.transactions[idx_to_execute as usize];

        let db = InstrumentedDB::new(base_view, latest_view, idx_to_execute);

        let mut executor = executor
            .modify()
            .reset_handler_with_db(db)
            .modify_tx_env(|etx| {
                etx.caller = Address::from(tx.from.as_fixed_bytes());
                etx.gas_limit = tx.gas.as_u64();
                local_fill!(etx.gas_price, tx.gas_price, U256::from_limbs);
                local_fill!(etx.value, Some(tx.value), U256::from_limbs);
                etx.data = tx.input.0.clone().into(); // Clone the input before converting
                let mut gas_priority_fee = U256::ZERO;
                local_fill!(
                    gas_priority_fee,
                    tx.max_priority_fee_per_gas,
                    U256::from_limbs
                );
                etx.gas_priority_fee = Some(gas_priority_fee);
                etx.chain_id = Some(chain_id);
                etx.nonce = Some(tx.nonce.as_u64());
                if let Some(ref access_list) = tx.access_list {
                    // Borrow access_list instead of moving
                    etx.access_list = access_list
                        .0
                        .iter() // Use iter() to avoid moving
                        .map(|item| {
                            let new_keys: Vec<U256> = item
                                .storage_keys
                                .iter() // Use iter() to avoid moving
                                .map(|h256| U256::from_le_bytes(h256.0))
                                .collect();
                            (Address::from(item.address.as_fixed_bytes()), new_keys)
                        })
                        .collect();
                } else {
                    etx.access_list = Default::default();
                }

                etx.transact_to = match tx.to {
                    Some(to_address) => {
                        TransactTo::Call(Address::from(to_address.as_fixed_bytes()))
                    }
                    None => TransactTo::create(),
                };
            })
            .build();
        let _ = executor.context.evm.journaled_state.finalize();

        let execute_result = executor.transact();

        let mut prev_modified_keys = last_input_output
            .modified_keys(idx_to_execute) // TODO standardize txn_idx
            .map_or(HashMap::new(), |keys| keys.collect());
        // Note: In aptos, this is mutable because of some cases where a special error occurs.
        // Need to check if the same error can occur here.
        let mut read_set = executor.db().take_parallel_reads();

        let mut updates_outside = false;
        let mut apply_updates = |output: &ResultAndState| {
            // TODO convert this to be an arced_write_set
            let mut updates = Vec::<(ReadKey, ReadValue)>::new();
            for (k, v) in &output.state {
                let miner = Address::from(
                    block
                        .author
                        .expect("Block author must be present")
                        .as_fixed_bytes(),
                );
                if *k == miner {
                    continue;
                }
                // Note: we are always assuming the accountinfo will be written to.
                // this will be a huge performance hit if it is not the case,
                // because transactions will often interact with the same account
                // while only modifying storage slots?
                updates.push((
                    ReadKey::Basic(k.clone()),
                    ReadValue::Basic(Some(v.info.clone())),
                ));

                for (ks, vs) in v.changed_storage_slots() {
                    updates.push((
                        ReadKey::Storage(k.clone(), ks.clone()),
                        ReadValue::Storage(vs.present_value.clone()),
                    ));
                }
            }

            for (k, v) in &updates {
                if prev_modified_keys.remove(k).is_none() {
                    updates_outside = true;
                }
                versioned_cache
                    // TODO: should remove these clones?
                    .data()
                    .write(k.clone(), idx_to_execute, incarnation, v.clone());
            }
            updates
        };

        let (result, write_set) = match execute_result {
            Ok(output) => {
                let write_set = apply_updates(&output);
                (output, write_set)
            }
            Err(EVMError::Transaction(InvalidTransaction::NonceTooHigh { .. })) => {
                #[cfg(debug_assertions)]
                println!("nonce too high error {:?}", std::thread::current().id());
                read_set.mark_failure();
                use revm::primitives::{ExecutionResult, HaltReason, State};
                let output = ResultAndState {
                    // TODO: this is dummy, replace
                    result: ExecutionResult::Halt {
                        reason: HaltReason::NonceOverflow,
                        gas_used: 0,
                    },
                    state: State::default(),
                };
                (output, Vec::new())
            }
            Err(error) => {
                // Convert the error to the custom error type and box it
                let execution_error = Box::new(ExecutionError(format!("{:?}", error)));
                dbg!("Error in executor.rs::execute: {}", execution_error);
                return Err(code_invariant_error(
                    "actually no invariant broken, revm just failed execution",
                ));
            }
        };

        // Remove entries from previous write/delta set that were not overwritten.
        for k in prev_modified_keys.keys() {
            versioned_cache.data().remove(k, idx_to_execute);
        }
        // TODO figure out the difference between output and write set
        last_input_output.record(idx_to_execute, read_set, result, write_set);

        Ok((updates_outside, executor))
    }

    fn validate(
        idx_to_validate: TxnIndex,
        last_input_output: &TxnLastInputOutput,
        versioned_cache: &MVHashMap,
    ) -> Result<bool, PanicError> {
        #[cfg(debug_assertions)]
        println!(
            "Validating transaction {} on thread {:?}",
            idx_to_validate,
            std::thread::current().id()
        );

        let read_set = last_input_output
            .read_set(idx_to_validate)
            .expect("[BlockSTM]: Prior read-set must be recorded");

        if read_set.is_incorrect_use() {
            return Err(code_invariant_error(
                "Incorrect use detected in CapturedReads",
            ));
        }

        let is_valid = read_set.validate_reads(versioned_cache.data(), idx_to_validate);
        Ok(is_valid) // TODO: finish
    }

    fn update_transaction_on_abort(
        txn_idx: TxnIndex,
        last_input_output: &TxnLastInputOutput,
        versioned_cache: &MVHashMap,
    ) {
        // Mark the latest write/delta sets as estimates if the transaction is aborted.
        if let Some(keys) = last_input_output.modified_keys(txn_idx) {
            for (k, _) in keys {
                versioned_cache.data().mark_estimate(&k, txn_idx);
            }
        }
    }

    fn update_on_validation(
        txn_idx: TxnIndex,
        incarnation: Incarnation,
        valid: bool,
        validation_wave: Wave,
        last_input_output: &TxnLastInputOutput,
        versioned_cache: &MVHashMap,
        scheduler: &Scheduler,
    ) -> Result<SchedulerTask, PanicError> {
        let aborted = !valid && scheduler.try_abort(txn_idx, incarnation);

        if aborted {
            Self::update_transaction_on_abort(txn_idx, last_input_output, versioned_cache);
            scheduler.finish_abort(txn_idx, incarnation)
        } else {
            scheduler.finish_validation(txn_idx, validation_wave);
            if valid {
                scheduler.queueing_commits_arm();
            }

            Ok(SchedulerTask::Retry)
        }
    }

    /*
    Note: this function in aptos basically handles some
    revalidation associated with delayed fields and then
    does some final group processing. however, none of this
    should apply to us. We should find a way to directly commit,
    we don't need to prepare and queue, if they're commitable they
    should just be commited in parallel and not locked like this.
     */
    fn prepare_and_queue_commit_ready_txns(
        &self,
        scheduler: &Scheduler,
        scheduler_task: &mut SchedulerTask,
    ) {
        while let Some((txn_idx, _)) = scheduler.try_commit() {
            // TODO understand why this defer statement exists in blockstm
            defer! {
                scheduler.add_to_commit_queue(txn_idx);
            }

            if txn_idx + 1 == scheduler.num_txns() {
                assert!(!matches!(
                    scheduler_task,
                    SchedulerTask::ExecutionTask(_, _, _)
                ));

                scheduler.halt();
            }
        }
    }

    fn materialize_txn_commit(
        &self,
        txn_idx: TxnIndex,
        last_input_output: &TxnLastInputOutput,
        final_results: &ExplicitSyncWrapper<Vec<Option<ResultAndState>>>,
    ) -> Result<(), PanicError> {
        let mut final_results = final_results.acquire();
        final_results[txn_idx as usize] = Some(last_input_output.take_output(txn_idx));
        Ok(())
    }

    fn worker_loop(
        &self,
        block: &Block<Transaction>,
        last_input_output: &TxnLastInputOutput,
        versioned_cache: &MVHashMap,
        scheduler: &Scheduler,
        base_view: &DB,
        chain_id: u64,
        final_results: &ExplicitSyncWrapper<Vec<Option<ResultAndState>>>,
    ) -> Result<(), PanicError> {
        let mut scheduler_task = SchedulerTask::Retry;

        let drain_commit_queue = || -> Result<(), PanicError> {
            while let Ok(txn_idx) = scheduler.pop_from_commit_queue() {
                self.materialize_txn_commit(txn_idx, last_input_output, final_results)?;
            }
            Ok(())
        };

        // TODO: move this back out of the loop without reintroducing cache issues
        // TODO: explore not building it here -> passing in evmbuilder
        // and finish building it there.
        let mut executor = Evm::builder()
            .with_db(InstrumentedDB::new(
                base_view,
                ParallelState::new(versioned_cache, scheduler),
                0,
            ))
            .modify_block_env(|b| {
                if let Some(number) = block.number {
                    let nn = number.0[0];
                    b.number = U256::from(nn);
                }
                local_fill!(b.coinbase, block.author);
                local_fill!(b.timestamp, Some(block.timestamp), U256::from_limbs);
                local_fill!(b.difficulty, Some(block.difficulty), U256::from_limbs);
                local_fill!(b.gas_limit, Some(block.gas_limit), U256::from_limbs);
                if let Some(base_fee) = block.base_fee_per_gas {
                    local_fill!(b.basefee, Some(base_fee), U256::from_limbs);
                }
            })
            .modify_cfg_env(|c| {
                c.chain_id = chain_id;
            })
            .build();

        loop {
            while scheduler.should_coordinate_commits() {
                self.prepare_and_queue_commit_ready_txns(scheduler, &mut scheduler_task);
                scheduler.queueing_commits_mark_done();
            }

            drain_commit_queue()?;

            scheduler_task = match scheduler_task {
                SchedulerTask::ValidationTask(txn_idx, incarnation, wave) => {
                    let valid = Self::validate(txn_idx, last_input_output, versioned_cache)?;
                    #[cfg(debug_assertions)]
                    println!(
                        "txn_idx = {}, valid = {}, {:?}",
                        txn_idx,
                        valid,
                        std::thread::current().id()
                    );
                    Self::update_on_validation(
                        txn_idx,
                        incarnation,
                        valid,
                        wave,
                        last_input_output,
                        versioned_cache,
                        scheduler,
                    )?
                }
                SchedulerTask::ExecutionTask(
                    txn_idx,
                    incarnation,
                    ExecutionTaskType::Execution,
                ) => {
                    let (updates_outside, new_executor) = Self::execute(
                        txn_idx,
                        incarnation,
                        block,
                        last_input_output,
                        versioned_cache,
                        executor,
                        base_view,
                        ParallelState::new(versioned_cache, scheduler),
                        chain_id,
                    )?;
                    executor = new_executor;
                    scheduler.finish_execution(txn_idx, incarnation, updates_outside)?
                }
                SchedulerTask::ExecutionTask(_, _, ExecutionTaskType::Wakeup(condvar)) => {
                    {
                        let (lock, cvar) = &*condvar;

                        // Mark dependency resolved.
                        let mut lock = lock.lock();
                        *lock = DependencyStatus::Resolved;
                        // Wake up the process waiting for dependency.
                        cvar.notify_one();
                    }

                    scheduler.next_task()
                }
                SchedulerTask::Retry => scheduler.next_task(),
                SchedulerTask::Done => {
                    break Ok(());
                }
            }
        }
    }

    pub(crate) fn execute_transactions_parallel(
        &self,
        block: &Block<Transaction>,
        base_view: &DB,
        chain_id: u64,
    ) -> Result<Vec<Option<ResultAndState>>, PanicError> {
        // Using parallel execution with 1 thread currently will not work as it
        // will only have a coordinator role but no workers for rolling commit.
        // Need to special case no roles (commit hook by thread itself) to run
        // w. concurrency_level = 1 for some reason.
        // assert!(self.concurrency_level > 1, "Must use sequential execution"); // TODO temporarily commented

        let versioned_cache = MVHashMap::new();

        let num_txns = block.transactions.len();

        let final_results = ExplicitSyncWrapper::new(Vec::with_capacity(num_txns));

        {
            final_results.acquire().resize_with(num_txns, || None);
        }

        let num_txns = num_txns as u32;

        let last_input_output = TxnLastInputOutput::new(num_txns);
        let scheduler = Scheduler::new(num_txns);

        self.executor_thread_pool.scope(|s| {
            for _ in 0..self.concurrency_level {
                s.spawn(|_| {
                    self.worker_loop(
                        &block,
                        &last_input_output,
                        &versioned_cache,
                        &scheduler,
                        base_view,
                        chain_id,
                        &final_results,
                    )
                    .expect("worker loop fucked up") // TODO handle more gracefully
                })
            }
        });

        Ok(final_results.into_inner())
    }

    pub fn execute_block(
        &self,
        block: &Block<Transaction>,
        base_view: &DB,
        chain_id: u64,
    ) -> Result<Vec<Option<ResultAndState>>, PanicError> {
        // if self.concurrency_level > 1 { // TODO temporarily commented
        //     // Note: the aptos execute_transactions_parallel splits the block
        //     // into the block and transactions as two seperate arguments
        return self.execute_transactions_parallel(block, base_view, chain_id);
        // }
    }
}
