Custom implementations for Single Producer Single Consumer (SPSC) Ring Buffer in Rust.

Use crossbeam for cache padding and atomic operations.
Use MaybeUninit for uninitialized memory handling -> better performance.
Lock-free implementation for high-performance scenarios.


## Running Tests
```rust
cargo test
```

## Running Benchmarks
```rust
cargo bench
```

## Profiling
To profile the ring buffer performance, use the `profile.rs` script located in the `src/bin` directory. You can run it using:
```rust
RUSTFLAGS="-C force-frame-pointers=yes" cargo build --bin profile --profile profiling
perf stat -e   cycles,instructions,branches,branch-misses   ./target/profiling/profile
perf stat -e cache-misses,cache-references,LLC-loads,LLC-load-misses target/profiling/profile
RUSTFLAGS="-C force-frame-pointers=yes" cargo flamegraph --bin profile --profile profiling
```
