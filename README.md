# BlockSTM

### How to run

First, generate a serialized cachedb for in memory reads 
```
cargo run --example load_db --features="example_utils"
```

Then, to execute the test, run the following:  
```
cargo run --example parallel --features="example_utils"
```

To compare execution of sequential vs parallel for correctness:  
```
cargo run --example compare --features="example_utils"
```

To test performance, add the `--release` flag like such 
```
cargo run --example compare --features="example_utils" --release
```

Profiling:
```
CARGO_PROFILE_RELEASE_DEBUG=true cargo flamegraph --root --release --example simple --features "example_utils"
```

TODOs:
**figure out why its so slow**
 - miner is partially causing it, all the transactions seem to write miner

fix error handling (incl spec exec error todos in view.rs)
fix cachedb for non sequential paths (panicing when state reads don't match)
figure out why one thread is slower than sequential execution by 50%.
How to differentiate between 'true' NonceTooHighError and 'speculative' NonceTooHighError?
modify versioned_data to take arcs instead of raw data to avoid cloning
organize functions alphabetically in files to match aptos structure
add unit tests
should ethers-providers and ethers-core be feature flagged?
organize cargo imports
 - I think we should match blockstm import style (no line breaks, alphabetical), unless cleaner to do otherwise?
note: the txnindex that revm uses is usize, while u32 is aptos.
determine which we should be using.
If our MVHashMap only uses VersionedData, just use VersionedData directly 
set up devops
 - precommit hook for building

figure out why block 10889443 is failing when miner is skipped in the write set


ideas:
r/w inspector instead?
study aptos aggregator and investigate whether or not aggregation and delayed fields is a performance improvement that can/should be included


