use blockstm::example_utils::example_utils::PlaceholderDB;
use blockstm::executor::BlockExecutor;
use clap::Parser;
use ethers_providers::Middleware;
use ethers_providers::{Http, Provider};
use revm::db::{CacheDB, DatabaseCommit, DatabaseRef};
use revm::primitives::{Account, AccountInfo, Address, Bytecode, TransactTo, B256, U256};
use revm::{Database, Evm};
use serde_json;
use std::collections::HashMap;
use std::fs::read_to_string;
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

fn load_db(block_number: u64) -> CacheDB<PlaceholderDB> {
    // Read the JSON content from the file with dynamic block number
    let file_path = format!("db/cache_db_{}.json", block_number);
    let data = read_to_string(file_path).expect("Failed to read file");
    // Deserialize it back into a CacheDB instance
    serde_json::from_str(&data).expect("Failed to deserialize")
}

#[derive(Parser, Debug)]
#[clap(version, about)]
struct Args {
    /// SSD read delay in microseconds
    #[clap(short = 'd')]
    ssd_delay_us: u64,

    /// Number of threads to use, defaults to number of logical cores
    #[clap(short = 't', default_value_t = num_cpus::get())]
    num_threads: usize,

    /// If set, also executes in sequential mode
    #[clap(short = 's')]
    sequential: bool,
}

// define a new type that inherits from CacheDB and overrides the storage method
#[derive(Clone)]
pub struct SlowCacheDB<CacheDB> {
    inner: CacheDB,
    delay: std::time::Duration,
}

impl DatabaseRef for SlowCacheDB<CacheDB<PlaceholderDB>> {
    type Error = <CacheDB<PlaceholderDB> as DatabaseRef>::Error;

    #[doc = " Get basic account information."]
    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.inner.basic_ref(address)
    }

    #[doc = " Get account code by its hash."]
    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.inner.code_by_hash_ref(code_hash)
    }

    #[doc = " Get storage value of address at index."]
    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        // Add a delay to mimic the effects of storage overhead
        std::thread::sleep(self.delay);

        self.inner.storage_ref(address, index)
    }

    #[doc = " Get block hash by block number."]
    fn block_hash_ref(&self, number: U256) -> Result<B256, Self::Error> {
        self.inner.block_hash_ref(number)
    }
}

impl Database for SlowCacheDB<CacheDB<PlaceholderDB>> {
    type Error = <CacheDB<PlaceholderDB> as DatabaseRef>::Error;

    #[doc = " Get basic account information."]
    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.inner.basic(address)
    }

    #[doc = " Get account code by its hash."]
    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.inner.code_by_hash(code_hash)
    }

    #[doc = " Get storage value of address at index."]
    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        // Add a delay to mimic the effects of storage overhead
        std::thread::sleep(self.delay);

        self.inner.storage(address, index)
    }

    #[doc = " Get block hash by block number."]
    fn block_hash(&mut self, number: U256) -> Result<B256, Self::Error> {
        self.inner.block_hash(number)
    }
}

impl DatabaseCommit for SlowCacheDB<CacheDB<PlaceholderDB>> {
    #[doc = " Commit changes to the database."]
    fn commit(&mut self, changes: HashMap<Address, Account>) {
        self.inner.commit(changes)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let ssd_delay = std::time::Duration::from_micros(args.ssd_delay_us);
    println!("SSD read delay: {:?}μs", args.ssd_delay_us);

    // Create ethers client and wrap it in Arc<M>
    let client = Provider::<Http>::try_from(
        "https://eth-mainnet.g.alchemy.com/v2/W4x3U0VYrSTq3lqxWGcMn39sKx05Yfee",
    )?;
    let client = Arc::new(client);

    // Params
    let chain_id: u64 = 1;
    let block_number = 10889447;

    // Fetch the transaction-rich block
    let mut block = match client.get_block_with_txs(block_number).await {
        Ok(Some(block)) => block,
        Ok(None) => anyhow::bail!("Block not found"),
        Err(error) => anyhow::bail!("Error: {:?}", error),
    };
    println!("Fetched block number: {}", block.number.unwrap().0[0]);
    let txs = block.transactions.len();
    println!("Found {txs} transactions.");

    let orig_cache_db = load_db(block_number);
    let mut cache_db = SlowCacheDB {
        inner: orig_cache_db.clone(),
        delay: ssd_delay,
    };

    let num_trials = 100;
    println!("Number of trials: {}", num_trials);
    // println!("Starting parallel execution...");
    let parallel_time;
    let mut par_res = vec![];
    {
        // let num_threads = 1;
        let num_threads = args.num_threads;
        let mut total_duration = std::time::Instant::now().elapsed();
        let start_time = std::time::Instant::now();
        for _ in 0..num_trials {
            let executor_thread_pool = Arc::new(
                rayon::ThreadPoolBuilder::new()
                    .num_threads(num_threads)
                    .build()
                    .unwrap(),
            );
            let executor = BlockExecutor::new(num_threads, executor_thread_pool);
            let start_time = std::time::Instant::now();
            par_res = executor.execute_block(&block, &cache_db, chain_id).unwrap();
            let duration = start_time.elapsed();
            total_duration += duration;
        }
        let _duration = start_time.elapsed();
        parallel_time = total_duration / num_trials;
        println!(
            "Parallel execution time with {:?} threads: {:?}",
            num_threads, parallel_time
        );
    }

    assert!(
        par_res.iter().all(Option::is_some),
        "All elements in par_res must be Some"
    );

    let total_transactions = par_res.len();
    let tps = total_transactions as f64 / parallel_time.as_secs_f64();
    println!("Transactions per second (TPS): {:.2}", tps);

    // println!("Starting sequential execution...");
    if args.sequential {
        let sequential_time;
        let mut seq_res = vec![None; block.transactions.len()];
        {
            let block_clone = block.clone();
            let cache_db_clone = cache_db.clone();
            let start_time = std::time::Instant::now();
            for _ in 0..num_trials {
                block = block_clone.clone();
                cache_db = cache_db_clone.clone();
                let mut evm = Evm::builder()
                    .with_db(&mut cache_db)
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

                // Fill in CfgEnv
                for (index, tx) in block.transactions.into_iter().enumerate() {
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

                    let execute_result = evm.transact().unwrap();
                    evm.context.evm.db.commit(execute_result.clone().state); // note this shouldn't be timed
                    seq_res[index] = Some(execute_result);
                }
            }
            let duration = start_time.elapsed();
            sequential_time = duration / num_trials;
            println!("Sequential execution time: {:?}", sequential_time);
        }

        let speed_up_percentage = ((sequential_time.as_secs_f64() - parallel_time.as_secs_f64())
            / sequential_time.as_secs_f64())
            * 100.0;
        let speed_up_multiplier = sequential_time.as_secs_f64() / parallel_time.as_secs_f64();
        println!(
            "Parallel execution is {:.2}% faster than sequential execution.",
            speed_up_percentage
        );
        println!(
            "Parallel execution is {:.2}x faster than sequential execution.",
            speed_up_multiplier
        );

        assert!(
            seq_res.iter().all(Option::is_some),
            "All elements in seq_res must be Some"
        );

        let all_elements_match = par_res
            .iter()
            .zip(seq_res.iter())
            .all(|(par, seq)| par == seq);

        assert!(
            all_elements_match,
            "Elements in par_res and seq_res do not match."
        );

        let total_transactions = seq_res.len();
        let tps = total_transactions as f64 / sequential_time.as_secs_f64();
        println!("Sequential transactions per second (STPS): {:.2}", tps);
    }
    Ok(())
}
