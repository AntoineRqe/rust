# Rust Performance Profiling with perf & Flamegraphs

Complete guide for profiling Rust applications using Linux perf and flamegraphs.

## Table of Contents
1. [Installation](#installation)
2. [Quick Start](#quick-start)
3. [Perf Basics](#perf-basics)
4. [Flamegraph Generation](#flamegraph-generation)
5. [Cargo Flamegraph](#cargo-flamegraph)
6. [Advanced Usage](#advanced-usage)
7. [Troubleshooting](#troubleshooting)

---

## Installation

### Install perf (Linux)
```bash
# Ubuntu/Debian
sudo apt-get install linux-tools-common linux-tools-generic linux-tools-`uname -r`

# Fedora/RHEL
sudo dnf install perf

# Arch Linux
sudo pacman -S perf

# Verify installation
perf --version
```

### Install Flamegraph Tools

**Option 1: cargo-flamegraph (Easiest)**
```bash
cargo install flamegraph
```

**Option 2: Manual flamegraph.pl**
```bash
git clone https://github.com/brendangregg/FlameGraph
cd FlameGraph
# Add to PATH
echo 'export PATH="$PATH:$HOME/FlameGraph"' >> ~/.bashrc
source ~/.bashrc
```

### Enable Debug Symbols in Release Mode

Edit your `Cargo.toml`:
```toml
[profile.release]
debug = true              # Enable debug symbols
strip = false             # Don't strip symbols
```

Or create a custom profile:
```toml
[profile.profiling]
inherits = "release"
debug = true
strip = false
```

---

## Quick Start

### Method 1: Using cargo-flamegraph (Recommended)
```bash
# Profile your binary
cargo flamegraph

# Profile with release optimizations
cargo flamegraph --release

# Profile specific binary
cargo flamegraph --bin your-binary-name

# Profile with custom frequency (1000 Hz)
cargo flamegraph --freq 1000

# Profile specific package in workspace
cargo flamegraph -p llm --bin llm-test
```

Output: `flamegraph.svg` in your project root

### Method 2: Manual perf + flamegraph
```bash
# Build with debug symbols
cargo build --release

# Record performance data
sudo perf record -F 99 -g --call-graph dwarf ./target/release/your-binary

# Generate flamegraph
perf script | stackcollapse-perf.pl | flamegraph.pl > flamegraph.svg

# Open in browser
firefox flamegraph.svg
```

---

## Perf Basics

### Recording Performance Data

**Basic recording:**
```bash
# Record with default settings
sudo perf record ./target/release/your-binary

# Record with specific frequency (99 Hz recommended)
sudo perf record -F 99 -g ./target/release/your-binary

# Record with call graph (better stack traces)
sudo perf record -F 99 -g --call-graph dwarf ./target/release/your-binary

# Record for specific duration
sudo perf record -F 99 -g --call-graph dwarf -- sleep 30 &
./target/release/your-binary
```

**Recording options:**
```bash
# -F: Sampling frequency (Hz)
#     99 = sample 99 times per second (recommended)
#     999 = higher frequency, more overhead
#     4000 = very detailed, high overhead

# -g: Enable call-graph (stack trace) recording
# --call-graph dwarf: Use DWARF debug info for stack traces
# --call-graph fp: Use frame pointers (faster, less accurate)

# -e: Specify events to record
sudo perf record -e cycles,instructions ./target/release/your-binary

# Record all CPUs
sudo perf record -a -g ./target/release/your-binary

# Record specific process
sudo perf record -p <PID> -g

# Record with output file
sudo perf record -o mydata.perf -F 99 -g ./target/release/your-binary
```

### Analyzing Perf Data

**View report:**
```bash
# Interactive TUI report
sudo perf report

# Text-based report
sudo perf report --stdio

# Show top functions
sudo perf report --stdio --sort comm,dso,symbol | head -30
```

**Navigation in perf report TUI:**
- `Arrow keys`: Navigate
- `Enter`: Zoom into function
- `Esc`: Zoom out
- `a`: Annotate assembly
- `h`: Help
- `q`: Quit

**Generate various outputs:**
```bash
# Generate script output (for flamegraph)
sudo perf script > out.perf

# Generate with full context
sudo perf script -F comm,tid,pid,time,event,ip,sym,dso > out.perf

# Show statistics
sudo perf stat ./target/release/your-binary
```

---

## Flamegraph Generation

### Using cargo-flamegraph

**Basic usage:**
```bash
# Generate flamegraph for main binary
cargo flamegraph

# With release optimizations
cargo flamegraph --release

# Specify custom output
cargo flamegraph -o profile.svg

# Profile tests
cargo flamegraph --test test_name

# Profile benchmarks
cargo flamegraph --bench bench_name
```

**Advanced cargo-flamegraph:**
```bash
# Higher sampling frequency
cargo flamegraph --freq 1000

# Profile specific features
cargo flamegraph --features "feature1,feature2"

# Profile with arguments
cargo flamegraph -- --your-app-arg1 --arg2

# Profile for longer duration
cargo flamegraph -- --run-time 60

# Custom profiling profile
cargo flamegraph --profile profiling
```

### Manual Flamegraph Pipeline

**Complete workflow:**
```bash
# 1. Build with debug symbols
cargo build --release

# 2. Record performance data
sudo perf record -F 99 -g --call-graph dwarf ./target/release/your-binary

# 3. Extract perf script
sudo perf script > out.perf

# 4. Collapse stacks
stackcollapse-perf.pl out.perf > out.folded

# 5. Generate flamegraph
flamegraph.pl out.folded > flamegraph.svg

# 6. View in browser
firefox flamegraph.svg
```

**One-liner:**
```bash
sudo perf record -F 99 -g --call-graph dwarf ./target/release/your-binary && \
sudo perf script | stackcollapse-perf.pl | flamegraph.pl > flamegraph.svg
```

### Flamegraph Variants

**Different flamegraph types:**
```bash
# Standard CPU flamegraph
flamegraph.pl out.folded > cpu.svg

# Inverted (icicle graph)
flamegraph.pl --inverted out.folded > icicle.svg

# Differential flamegraph (compare two profiles)
difffolded.pl out1.folded out2.folded | flamegraph.pl > diff.svg

# Memory flamegraph (requires custom events)
perf record -e page-faults -g ./target/release/your-binary
perf script | stackcollapse-perf.pl | flamegraph.pl > memory.svg
```

---

## Cargo Flamegraph

### Installation & Setup

```bash
# Install
cargo install flamegraph

# Verify
cargo flamegraph --version
```

### Common Workflows

**Development cycle:**
```bash
# 1. Profile development build (fast iteration)
cargo flamegraph

# 2. Profile release build (realistic performance)
cargo flamegraph --release

# 3. Profile specific scenario
cargo flamegraph --release -- --input large_file.json
```

**Workspace profiling:**
```bash
# Profile specific package
cargo flamegraph -p llm

# Profile specific binary in workspace
cargo flamegraph -p cli --bin my-app

# Profile with all features
cargo flamegraph --all-features
```

**Integration tests:**
```bash
# Profile integration test
cargo flamegraph --test integration_test_name

# Profile with test arguments
cargo flamegraph --test my_test -- --nocapture
```

### Configuration

Create `.cargo/config.toml`:
```toml
[target.x86_64-unknown-linux-gnu]
linker = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=lld"]

[profile.profiling]
inherits = "release"
debug = true
strip = false
```

---

## Advanced Usage

### Profiling Specific Scenarios

**Profile specific function:**
```rust
// In your code
fn main() {
    // Warmup
    for _ in 0..10 {
        expensive_operation();
    }
    
    // Profile this part
    for _ in 0..1000 {
        expensive_operation();
    }
}
```

**Profile with workload:**
```bash
# Create a script to generate load
cat > workload.sh << 'EOF'
#!/bin/bash
for i in {1..100}; do
    curl http://localhost:8080/api/endpoint
    sleep 0.1
done
EOF

chmod +x workload.sh

# Start profiling
sudo perf record -F 99 -g -p $(pgrep your-binary) &
PERF_PID=$!

# Run workload
./workload.sh

# Stop profiling
sudo kill -INT $PERF_PID

# Generate flamegraph
sudo perf script | stackcollapse-perf.pl | flamegraph.pl > flamegraph.svg
```

### System-Wide Profiling

```bash
# Profile all processes on system
sudo perf record -a -g -F 99 -- sleep 30

# Profile specific CPU cores
sudo perf record -C 0,1 -g -F 99 -- sleep 30

# Profile with multiple events
sudo perf record -e cycles,cache-misses,branch-misses -a -g -- sleep 30
```

### Memory Profiling

**Using massif (Valgrind):**
```bash
# Install valgrind
sudo apt-get install valgrind

# Run memory profiling
valgrind --tool=massif --massif-out-file=massif.out ./target/release/your-binary

# Visualize
ms_print massif.out > massif.txt
less massif.txt
```

**Using heaptrack:**
```bash
# Install
sudo apt-get install heaptrack

# Profile
heaptrack ./target/release/your-binary

# Analyze
heaptrack_gui heaptrack.your-binary.*.gz
```

### Off-CPU Profiling

**Profile time waiting (I/O, locks):**
```bash
# Record scheduler events
sudo perf record -e sched:sched_switch -e sched:sched_stat_sleep \
    -e sched:sched_stat_blocked -a -g -- sleep 30

# Or use offcputime.bt (bpftrace)
sudo bpftrace -e 'tracepoint:sched:sched_switch { @[kstack, ustack, comm] = count(); }'
```

---

## Perf Event Types

### CPU Events
```bash
# CPU cycles
sudo perf record -e cycles ./target/release/your-binary

# Instructions executed
sudo perf record -e instructions ./target/release/your-binary

# Branch misses
sudo perf record -e branch-misses ./target/release/your-binary

# Cache misses
sudo perf record -e cache-misses ./target/release/your-binary
sudo perf record -e LLC-load-misses ./target/release/your-binary
```

### List Available Events
```bash
# List all events
perf list

# List hardware events
perf list hardware

# List software events
perf list software

# List cache events
perf list cache

# Search for specific events
perf list | grep -i cache
```

---

## Reading Flamegraphs

### Understanding the Visualization

```
┌──────────────────────────────────────────────────────┐
│                    main()                             │  ← Bottom: Program entry
├─────────────────┬────────────────────────────────────┤
│   init()        │         run_app()                   │  ← Functions called by main
├────┬────────────┼──────────┬─────────────────────────┤
│db  │   config   │ process  │      network            │  ← Subfunctions
└────┴────────────┴──────────┴─────────────────────────┘
     ↑                         ↑
   Narrow                    Wide
  (less time)              (more time)
```

**Key points:**
- **Width**: Proportional to time spent (wider = more CPU time)
- **Color**: Usually random (just for distinction), sometimes red=hot
- **X-axis**: Alphabetical, NOT time-based
- **Y-axis**: Call stack depth (bottom = entry, top = leaf functions)

### What to Look For

**Hot paths:**
```
Wide boxes = CPU hotspots → Focus optimization here
```

**Tall stacks:**
```
Deep call chains = potential optimization target
Consider: inlining, reducing abstraction overhead
```

**Platform code:**
```
Large boxes with names like `__libc_start`, `clone` → System overhead
Usually not your code, but good to know
```

---

## Optimization Workflow

### 1. Baseline Measurement
```bash
# Record baseline
cargo flamegraph --release -o baseline.svg

# Note: Save output metrics
time ./target/release/your-binary
```

### 2. Identify Hotspots
```bash
# Open flamegraph
firefox baseline.svg

# Look for:
# - Widest boxes (most CPU time)
# - Repeated patterns
# - Unexpected function calls
```

### 3. Optimize
```rust
// Example: Found hotspot in JSON parsing
// Before:
let data: MyStruct = serde_json::from_str(&json)?;

// After: Use from_reader for better performance
let data: MyStruct = serde_json::from_reader(reader)?;
```

### 4. Measure Improvement
```bash
# Generate new flamegraph
cargo flamegraph --release -o optimized.svg

# Compare
firefox baseline.svg optimized.svg

# Benchmark the difference
hyperfine './target/release/your-binary-before' './target/release/your-binary-after'
```

### 5. Differential Flamegraph
```bash
# Create differential flamegraph
sudo perf record -F 99 -g ./target/release/your-binary-before
sudo perf script > before.perf
stackcollapse-perf.pl before.perf > before.folded

sudo perf record -F 99 -g ./target/release/your-binary-after
sudo perf script > after.perf
stackcollapse-perf.pl after.perf > after.folded

# Generate diff (red = regression, blue = improvement)
difffolded.pl before.folded after.folded | flamegraph.pl > diff.svg
```

---

## Troubleshooting

### Permission Errors

**Error: "perf_event_open() failed"**
```bash
# Temporary fix
sudo sh -c 'echo 1 >/proc/sys/kernel/perf_event_paranoid'

# Or use less restrictive setting
sudo sh -c 'echo -1 >/proc/sys/kernel/perf_event_paranoid'

# Permanent fix (add to /etc/sysctl.conf)
echo 'kernel.perf_event_paranoid = 1' | sudo tee -a /etc/sysctl.conf
sudo sysctl -p
```

**Error: "Permission denied"**
```bash
# Add user to perf_users group (if it exists)
sudo groupadd perf_users
sudo usermod -a -G perf_users $USER

# Or run with sudo
sudo -E cargo flamegraph
```

### Missing Debug Symbols

**Symptoms:**
- Flamegraph shows `<unknown>` or hex addresses
- Missing function names

**Fix:**
```toml
# In Cargo.toml
[profile.release]
debug = true
strip = false

# Or use custom profile
[profile.profiling]
inherits = "release"
debug = true
```

```bash
# Rebuild
cargo clean
cargo build --profile profiling
```

### Inlining Issues

**Problem:** Functions inlined by compiler don't show up

**Solutions:**
```rust
// Prevent inlining for profiling
#[inline(never)]
fn important_function() {
    // ...
}

// Or in Cargo.toml (less aggressive optimization)
[profile.profiling]
inherits = "release"
debug = true
opt-level = 2  # Instead of 3
lto = false    # Disable LTO for profiling
```

### Empty or Minimal Flamegraph

**Possible causes:**
1. Program runs too quickly
2. Sampling frequency too low
3. Not enough iterations

**Solutions:**
```bash
# Increase sampling frequency
cargo flamegraph --freq 999

# Make program run longer
cargo flamegraph -- --iterations 10000

# Profile in a loop
for i in {1..100}; do ./target/release/your-binary; done
```

### System-Specific Issues

**WSL2:**
```bash
# WSL2 doesn't support perf directly
# Use Docker with --privileged or native Linux
```

**macOS:**
```bash
# perf not available on macOS
# Use: cargo instruments instead
cargo install cargo-instruments
cargo instruments --template time
```

**Docker:**
```bash
# Run with --privileged and --pid=host
docker run --privileged --pid=host ...

# Or use --cap-add
docker run --cap-add=SYS_ADMIN ...
```

---

## Useful Scripts

### Auto-profile Script
```bash
#!/bin/bash
# auto_profile.sh - Automatically profile and open flamegraph

set -e

echo "Building with profiling symbols..."
cargo build --release

echo "Profiling..."
sudo perf record -F 99 -g --call-graph dwarf ./target/release/"$1"

echo "Generating flamegraph..."
sudo perf script | stackcollapse-perf.pl | flamegraph.pl > flamegraph.svg

echo "Cleaning up..."
sudo rm perf.data

echo "Opening flamegraph..."
xdg-open flamegraph.svg

echo "Done! Flamegraph saved to flamegraph.svg"
```

Usage:
```bash
chmod +x auto_profile.sh
./auto_profile.sh your-binary-name
```

### Compare Profiles Script
```bash
#!/bin/bash
# compare_profiles.sh - Compare two versions

if [ "$#" -ne 2 ]; then
    echo "Usage: $0 <binary1> <binary2>"
    exit 1
fi

echo "Profiling $1..."
sudo perf record -F 99 -g -o perf1.data "$1"
sudo perf script -i perf1.data | stackcollapse-perf.pl > profile1.folded

echo "Profiling $2..."
sudo perf record -F 99 -g -o perf2.data "$2"
sudo perf script -i perf2.data | stackcollapse-perf.pl > profile2.folded

echo "Generating differential flamegraph..."
difffolded.pl profile1.folded profile2.folded | flamegraph.pl > diff.svg

echo "Cleanup..."
sudo rm perf1.data perf2.data profile1.folded profile2.folded

echo "Opening differential flamegraph..."
xdg-open diff.svg
```

---

## Best Practices

### 1. Always Profile Release Builds
```bash
# ✅ Good
cargo flamegraph --release

# ❌ Bad (debug builds are too slow to be meaningful)
cargo flamegraph
```

### 2. Profile Real Workloads
```bash
# Use realistic data
cargo flamegraph --release -- --input real_data.json

# Not toy examples
cargo flamegraph --release -- --input small_test.json
```

### 3. Multiple Runs
```bash
# Run multiple times and compare
for i in {1..5}; do
    cargo flamegraph --release -o flamegraph_$i.svg
done
```

### 4. Focus on Hot Paths
- Profile first, optimize later
- Focus on functions taking >5% of total time
- Don't micro-optimize cold code

### 5. Measure, Don't Guess
```bash
# Always benchmark before and after
hyperfine --warmup 3 \
    './target/release/before' \
    './target/release/after'
```

---

## Additional Tools

### cargo-instruments (macOS)
```bash
cargo install cargo-instruments
cargo instruments --template time
```

### Criterion.rs (Microbenchmarks)
```bash
# Add to Cargo.toml
[dev-dependencies]
criterion = "0.5"

# Run benchmarks
cargo bench
```

### samply (Modern Alternative)
```bash
cargo install samply
samply record ./target/release/your-binary
```

### pprof (Google's Profiler)
```bash
cargo install pprof
cargo pprof --bin your-binary
```

---

## References

- [Brendan Gregg's Flamegraphs](http://www.brendangregg.com/flamegraphs.html)
- [Linux perf Examples](http://www.brendangregg.com/perf.html)
- [cargo-flamegraph GitHub](https://github.com/flamegraph-rs/flamegraph)
- [Rust Performance Book](https://nnethercote.github.io/perf-book/)