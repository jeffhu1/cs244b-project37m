# Concerto: Transaction-Parallel EVM

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


### Performance 

Compared to a single-threaded REVM instance, Concerto performs 4x faster when running with 6 threads

![TPS vs Parallelism](images/tps_vs_parallelism.jpg)
