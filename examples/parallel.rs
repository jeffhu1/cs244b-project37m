use blockstm::example_utils::example_utils::PlaceholderDB;
use blockstm::executor::BlockExecutor;

// loaded from revm generate_block_traces
use ethers_core::types::BlockId;
use ethers_providers::Middleware;
use ethers_providers::{Http, Provider};
use num_cpus;
use revm::db::{CacheDB, EthersDB};
use serde::{Deserialize, Serialize};
use serde_json;
use std::fs::read_to_string;
use std::sync::Arc;

fn load_db(block_number: u64) -> CacheDB<PlaceholderDB> {
    // Read the JSON content from the file with dynamic block number
    let file_path = format!("db/cache_db_{}.json", block_number);
    let data = read_to_string(file_path).expect("Failed to read file");
    // Deserialize it back into a CacheDB instance
    serde_json::from_str(&data).expect("Failed to deserialize")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("running block_stm tests");

    /* START PULL BLOCK USING ETHERS, rich with txs */
    // Create ethers client and wrap it in Arc<M>
    let client = Provider::<Http>::try_from(
        "https://eth-mainnet.g.alchemy.com/v2/W4x3U0VYrSTq3lqxWGcMn39sKx05Yfee",
    )?;
    let client = Arc::new(client);

    // Params
    let chain_id: u64 = 1;
    let block_number = 10889442;
    // let block_number = 10889447;
    // let block_number = 10889508;
    let num_threads = 1;
    let num_threads = 2;
    let num_threads = num_cpus::get();
    let use_cache_db = true; // Indicate whether to use the cache_db or state_db

    // Fetch the transaction-rich block
    let block = match client.get_block_with_txs(block_number).await {
        Ok(Some(block)) => block,
        Ok(None) => anyhow::bail!("Block not found"),
        Err(error) => anyhow::bail!("Error: {:?}", error),
    };
    println!("Fetched block number: {}", block.number.unwrap().0[0]);
    let previous_block_number = block_number - 1;

    let txs = block.transactions.len();
    println!("Found {txs} transactions.");

    /* END PULL BLOCK USING ETHERS */
    let executor_thread_pool = Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build()
            .unwrap(),
    );

    // Use the previous block state as the db with caching
    let prev_id: BlockId = previous_block_number.into();
    // SAFETY: This cannot fail since this is in the top-level tokio runtime
    if use_cache_db {
        let cache_db = load_db(block_number);
        let executor = BlockExecutor::new(num_threads, executor_thread_pool);
        let start_time = std::time::Instant::now();
        let _ = executor.execute_block(&block, &cache_db, chain_id);
        let elapsed_time = start_time.elapsed();
        println!("Execution with CacheDB took: {:?}", elapsed_time);
    } else {
        println!("Using regular db");
        let state_db = EthersDB::new(Arc::clone(&client), Some(prev_id)).expect("panic");
        let executor = BlockExecutor::new(num_threads, executor_thread_pool);
        let start_time = std::time::Instant::now();
        let _ = executor.execute_block(&block, &state_db, chain_id);
        let elapsed_time = start_time.elapsed();
        println!("Execution with EthersDB took: {:?}", elapsed_time);
    }

    Ok(())
}
