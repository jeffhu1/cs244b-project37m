use blockstm::example_utils::example_utils::BackedDB;
use ethers_core::types::BlockId;
use ethers_providers::Middleware;
use ethers_providers::{Http, Provider};
use indicatif::ProgressBar;
use revm::db::EthersDB;
use revm::primitives::{Address, TransactTo, U256};
use revm::Evm;
use serde_json;
use std::fs::File;
use std::io::Write;
use std::sync::Arc;

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
    let client = Arc::new(client);

    // Params
    let chain_id: u64 = 1;
    let block_number = 10889447;

    // Fetch the transaction-rich block
    let block = match client.get_block_with_txs(block_number).await {
        Ok(Some(block)) => block,
        Ok(None) => anyhow::bail!("Block not found"),
        Err(error) => anyhow::bail!("Error: {:?}", error),
    };
    println!("Fetched block number: {}", block.number.unwrap().0[0]);
    let previous_block_number = block_number - 1;

    // Use the previous block state as the db with caching
    let prev_id: BlockId = previous_block_number.into();
    // SAFETY: This cannot fail since this is in the top-level tokio runtime
    let state_db = EthersDB::new(Arc::clone(&client), Some(prev_id)).expect("panic");
    let mut backed_db = BackedDB::new(state_db);

    {
        let mut evm = Evm::builder()
            .with_db(&mut backed_db)
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

        // Fill in CfgEnv
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

            if let Err(error) = evm.transact_commit() {
                println!("Got error: {:?}", error);
            }

            console_bar.inc(1);
        }

        console_bar.finish_with_message("Finished all transactions.");
        println!(
            "Finished execution. Total CPU time: {:.6}s",
            elapsed.as_secs_f64()
        );
    }

    // Create the db directory if it doesn't exist
    std::fs::create_dir_all("db").expect("Failed to create traces directory");
    let serializable_db = backed_db.get_serializable_base_db();
    let serialized = serde_json::to_string(&serializable_db).expect("Failed to serialize CacheDB");
    // Write the JSON string to a file with block number included in the filename
    let file_name = format!("db/cache_db_{}.json", block_number);
    let mut file = File::create(&file_name).expect("Failed to create file");
    file.write_all(serialized.as_bytes())
        .expect("Failed to write to file");

    Ok(())
}
