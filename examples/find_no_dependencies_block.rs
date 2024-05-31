// Example Adapted From: https://github.com/bluealloy/revm/issues/672

use ethers_core::types::BlockId;
use ethers_providers::Middleware;
use ethers_providers::{Http, Provider};
use indicatif::ProgressBar;
use revm::db::{CacheDB, Database, DatabaseCommit, DatabaseRef, EthersDB, StateBuilder};
use revm::primitives::{Address, HashMap, TransactTo, U256};
use revm::{inspector_handle_register, Evm};
use std::fs::OpenOptions;
use std::io::BufWriter;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;

use revm::primitives::HashSet;

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create ethers client and wrap it in Arc<M>
    let client = Provider::<Http>::try_from(
        "https://eth-mainnet.g.alchemy.com/v2/W4x3U0VYrSTq3lqxWGcMn39sKx05Yfee",
    )?;
    // "https://mainnet.infura.io/v3/1e0b3d7c22d54d2b8dac3ab314110dae",
    let client = Arc::new(client);

    // Params
    let chain_id: u64 = 1;
    let mut block_number = 10889447;
    loop {
        block_number += 1;
        // Fetch the transaction-rich block
        let block = match client.get_block_with_txs(block_number).await {
            Ok(Some(block)) => block,
            Ok(None) => anyhow::bail!("Block not found"),
            Err(error) => anyhow::bail!("Error: {:?}", error),
        };
        println!("Fetched block number: {}", block.number.unwrap().0[0]);
        let previous_block_number = block_number - 1;

        let mut caller_frequencies: HashMap<Address, u32> = HashMap::new();
        for tx in &block.transactions {
            *caller_frequencies
                .entry(Address::from(tx.from.as_fixed_bytes()))
                .or_insert(0) += 1;
        }
        let frequent_callers: Vec<_> = caller_frequencies
            .iter()
            .filter(|&(_, &count)| count > 1)
            .collect();
        if !frequent_callers.is_empty() {
            println!(
                "Block number {} has {} callers with more than one transaction.",
                block_number,
                frequent_callers.len()
            );
            continue;
        }

        // Use the previous block state as the db with caching
        let prev_id: BlockId = previous_block_number.into();
        // SAFETY: This cannot fail since this is in the top-level tokio runtime
        let state_db = EthersDB::new(Arc::clone(&client), Some(prev_id)).expect("panic");
        let cache_db: CacheDB<EthersDB<Provider<Http>>> = CacheDB::new(state_db);
        let state = StateBuilder::new_with_database(cache_db).build();
        let mut evm = Evm::builder()
            .with_db(state)
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

        let txs = block.transactions.len();
        println!("Found {txs} transactions.");

        let console_bar = Arc::new(ProgressBar::new(txs as u64));
        let elapsed = std::time::Duration::ZERO;

        let mut success = true;
        let mut running_state = HashMap::new();
        if block.transactions.len() == 0 {
            continue;
        }
        let mut i = 0;
        for tx in block.transactions {
            evm = evm
                .modify()
                .modify_tx_env(|etx| {
                    etx.caller = Address::from(tx.from.as_fixed_bytes());
                    etx.gas_limit = tx.gas.as_u64();
                    local_fill!(etx.gas_price, tx.gas_price, U256::from_limbs);
                    local_fill!(etx.value, Some(tx.value), U256::from_limbs);
                    etx.data = tx.input.0.into();
                    let mut gas_priority_fee = U256::ZERO;
                    local_fill!(
                        gas_priority_fee,
                        tx.max_priority_fee_per_gas,
                        U256::from_limbs
                    );
                    etx.gas_priority_fee = Some(gas_priority_fee);
                    etx.chain_id = Some(chain_id);
                    etx.nonce = Some(tx.nonce.as_u64());
                    if let Some(access_list) = tx.access_list {
                        etx.access_list = access_list
                            .0
                            .into_iter()
                            .map(|item| {
                                let new_keys: Vec<U256> = item
                                    .storage_keys
                                    .into_iter()
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

            let ans = evm.transact().unwrap();
            for (key, value) in ans.state.clone() {
                if running_state.contains_key(&key) {
                    success = false;
                    // If the key already exists, you can handle it here. For example, you might want to merge or update the value.
                    // This is a placeholder for whatever logic you need to implement if the key exists.
                    // For example: running_state.entry(key).and_modify(|e| *e = value);
                }
                running_state.insert(key, value);
            }
            if !success {
                break;
            }
            evm.db_mut().commit(ans.state);

            console_bar.inc(1);
            dbg!(i);
            i += 1;
        }
        if success {
            println!("found block without conflicts {}", block_number);
            return Ok(());
        }
        println!("block {} had conflicts", block_number);
    }
}
