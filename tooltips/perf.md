# Perf & Flamegraph Quick Reference

## 🚀 Quick Start (30 seconds)

```bash
# Install
cargo install flamegraph

# Profile
cargo flamegraph --release

# Open flamegraph.svg in browser
```

---

## 📋 Common Commands

### cargo-flamegraph (Easiest)
```bash
cargo flamegraph                      # Profile debug build
cargo flamegraph --release            # Profile release build (recommended)
cargo flamegraph -p llm               # Profile specific package
cargo flamegraph --bin my-app         # Profile specific binary
cargo flamegraph --freq 999           # Higher sampling rate
cargo flamegraph -- --your-args       # Pass args to your binary
```

### Manual perf + flamegraph
```bash
# Record
sudo perf record -F 99 -g --call-graph dwarf ./target/release/my-app

# Generate flamegraph
sudo perf script | stackcollapse-perf.pl | flamegraph.pl > flamegraph.svg

# One-liner
sudo perf record -F 99 -g --call-graph dwarf ./target/release/my-app && \
sudo perf script | stackcollapse-perf.pl | flamegraph.pl > flame.svg
```

### perf Analysis
```bash
sudo perf report                      # Interactive TUI
sudo perf report --stdio              # Text output
sudo perf stat ./target/release/app   # Statistics
sudo perf list                        # Available events
```

---

## ⚙️ Setup

### Cargo.toml
```toml
[profile.release]
debug = true        # Enable debug symbols
strip = false       # Don't strip symbols

# Or custom profile
[profile.profiling]
inherits = "release"
debug = true
strip = false
```

### Fix Permissions
```bash
# Temporary
sudo sh -c 'echo 1 >/proc/sys/kernel/perf_event_paranoid'

# Permanent
echo 'kernel.perf_event_paranoid = 1' | sudo tee -a /etc/sysctl.conf
sudo sysctl -p
```

---

## 🔥 Flamegraph Reading

```
Width = CPU time (wider = slower)
Height = Call stack depth
X-axis = Alphabetical (NOT time!)
Y-axis = Stack depth (bottom = entry point)

Look for:
  • Wide boxes = hotspots 🎯
  • Tall stacks = deep call chains
  • Unexpected functions = bugs
```

---

## 💡 Common Workflows

### Profile & Optimize Loop
```bash
# 1. Baseline
cargo flamegraph --release -o before.svg
time ./target/release/my-app

# 2. Optimize code based on flamegraph

# 3. Compare
cargo flamegraph --release -o after.svg
time ./target/release/my-app

# 4. Diff
difffolded.pl before.folded after.folded | flamegraph.pl > diff.svg
```

### Profile Specific Scenario
```bash
# With specific input
cargo flamegraph --release -- --input large_file.json

# With iterations
cargo flamegraph --release -- --iterations 10000

# Profile test
cargo flamegraph --test my_test
```

### Profile Running Process
```bash
# Get PID
ps aux | grep my-app

# Profile for 30 seconds
sudo perf record -F 99 -g -p <PID> -- sleep 30

# Generate flamegraph
sudo perf script | stackcollapse-perf.pl | flamegraph.pl > flame.svg
```

---

## 🛠️ Troubleshooting

| Problem | Solution |
|---------|----------|
| Permission denied | `sudo sh -c 'echo 1 >/proc/sys/kernel/perf_event_paranoid'` |
| `<unknown>` in flamegraph | Add `debug = true` to `[profile.release]` |
| Empty flamegraph | Run longer or increase `--freq 999` |
| Functions missing | Disable inlining: `#[inline(never)]` |
| WSL2 not working | Use native Linux or Docker with `--privileged` |

---

## 📊 Useful perf Events

```bash
# CPU cycles
sudo perf record -e cycles ./target/release/app

# Cache misses
sudo perf record -e cache-misses ./target/release/app

# Branch misses
sudo perf record -e branch-misses ./target/release/app

# Multiple events
sudo perf record -e cycles,cache-misses,branch-misses ./target/release/app

# List all events
perf list
```

---

## 🎯 Optimization Tips

1. **Profile first** - Don't guess!
2. **Focus on hot paths** - Optimize widest boxes (>5% time)
3. **Use release builds** - Debug is too slow
4. **Real workloads** - Don't optimize toy examples
5. **Measure impact** - Use hyperfine to benchmark

```bash
# Compare performance
hyperfine --warmup 3 './target/release/before' './target/release/after'
```

---

## 🔧 Aliases (add to ~/.bashrc)

```bash
# Quick flamegraph
alias flame='cargo flamegraph --release'

# Profile with high frequency
alias flame-hf='cargo flamegraph --release --freq 999'

# Perf record shorthand
alias precord='sudo perf record -F 99 -g --call-graph dwarf'

# Perf report shorthand
alias preport='sudo perf report'

# Generate flamegraph from perf.data
alias pflame='sudo perf script | stackcollapse-perf.pl | flamegraph.pl > flamegraph.svg'
```

---

## 📦 Alternative Tools

```bash
# samply (modern, easier)
cargo install samply
samply record ./target/release/my-app

# cargo-instruments (macOS)
cargo install cargo-instruments
cargo instruments --template time

# Criterion (microbenchmarks)
cargo bench

# valgrind (memory profiling)
valgrind --tool=callgrind ./target/release/my-app
kcachegrind callgrind.out.*
```

---

## 📚 Quick Examples

### Example 1: Profile workspace crate
```bash
cd my-workspace
cargo flamegraph -p llm --release
```

### Example 2: Profile with arguments
```bash
cargo flamegraph --release -- process --file data.json --threads 4
```

### Example 3: Compare two branches
```bash
# Branch 1
git checkout feature-old
cargo build --release
sudo perf record -F 99 -g -o old.data ./target/release/app
sudo perf script -i old.data > old.perf

# Branch 2
git checkout feature-new
cargo build --release
sudo perf record -F 99 -g -o new.data ./target/release/app
sudo perf script -i new.data > new.perf

# Compare
stackcollapse-perf.pl old.perf > old.folded
stackcollapse-perf.pl new.perf > new.folded
difffolded.pl old.folded new.folded | flamegraph.pl > diff.svg
```

### Example 4: System-wide profiling
```bash
# Profile entire system for 30 seconds
sudo perf record -F 99 -a -g -- sleep 30
sudo perf script | stackcollapse-perf.pl | flamegraph.pl > system.svg
```

---

## ⚡ Performance Optimization Checklist

- [ ] Enable release mode optimizations
- [ ] Profile with realistic data
- [ ] Identify functions taking >5% CPU time
- [ ] Check for:
  - [ ] Unnecessary allocations
  - [ ] Excessive cloning
  - [ ] Inefficient algorithms
  - [ ] Lock contention
  - [ ] I/O bottlenecks
- [ ] Optimize hot paths first
- [ ] Benchmark before/after changes
- [ ] Generate differential flamegraph

---

## 🆘 Help Commands

```bash
cargo flamegraph --help
perf --help
perf record --help
perf report --help
perf list
```

## 🔗 Resources

- Brendan Gregg's site: http://www.brendangregg.com/flamegraphs.html
- cargo-flamegraph: https://github.com/flamegraph-rs/flamegraph
- Rust Performance Book: https://nnethercote.github.io/perf-book/