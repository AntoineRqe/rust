# sccache Monitoring & Usage Guide

## Installation

```bash
# Install sccache
cargo install sccache

# Or on Ubuntu/Debian
# sudo apt install sccache

# Or using cargo-binstall (faster)
cargo install cargo-binstall
cargo binstall sccache
```

## Setup

```bash
# Set sccache as your Rust compiler wrapper
export RUSTC_WRAPPER=sccache

# Make it permanent (add to ~/.bashrc or ~/.zshrc)
echo 'export RUSTC_WRAPPER=sccache' >> ~/.bashrc
source ~/.bashrc
```

## Essential Monitoring Commands

### 1. Show Statistics (Most Useful!)
```bash
# View cache hit/miss statistics
sccache --show-stats

# Short form
sccache -s
```

**Output example:**
```
Compile requests                    245
Compile requests executed           180
Cache hits                          150
Cache hits (C/C++)                   30
Cache hits (Rust)                   120
Cache misses                         30
Cache timeouts                        0
Cache read errors                     0
Forced recaches                       0
Cache write errors                    0
Compilation failures                  0
Cache errors                          0
Non-cacheable compilations            0
Non-cacheable calls                  65
Non-compilation calls                 0
Unsupported compiler calls            0
Average cache write               0.500 s
Average cache read miss           5.234 s
Average cache read hit            0.234 s
Failed distributed compilations       0

Cache location                  Local disk: "/home/user/.cache/sccache"
Cache size                           2.5 GB
Max cache size                      10.0 GB
```

### 2. Watch Stats in Real-Time
```bash
# Monitor sccache stats while building
watch -n 1 'sccache -s'

# Or using a loop
while true; do clear; sccache -s; sleep 2; done
```

### 3. Show Cache Hit Rate
```bash
# Calculate hit rate percentage
sccache -s | grep -E "Cache hits|requests executed"

# Or use this one-liner to show percentage
sccache -s | awk '/Compile requests executed/{exec=$4} /Cache hits [^(]/{hits=$3} END{if(exec>0) printf "Hit rate: %.1f%% (%d/%d)\n", (hits/exec)*100, hits, exec}'
```

### 4. Clear Cache Statistics
```bash
# Reset statistics counters (doesn't delete cache)
sccache --zero-stats

# Short form
sccache -z
```

### 5. Stop sccache Server
```bash
# Stop the background sccache server
sccache --stop-server
```

### 6. Start Fresh
```bash
# Clear stats and stop server (fresh start)
sccache --stop-server
sccache --zero-stats
```

## Cache Management Commands

### Check Cache Location and Size
```bash
# Show where cache is stored
sccache -s | grep "Cache location"

# Show current cache size
sccache -s | grep "Cache size"

# Manual check of cache directory
du -sh ~/.cache/sccache
```

### Clear/Delete Cache
```bash
# Delete all cached compilation artifacts
rm -rf ~/.cache/sccache

# Or if using custom location
rm -rf /path/to/your/cache
```

### Configure Cache Size
```bash
# Set max cache size (10GB example)
export SCCACHE_CACHE_SIZE="10G"

# Set custom cache directory
export SCCACHE_DIR="/path/to/cache"

# Add to ~/.bashrc for persistence
echo 'export SCCACHE_CACHE_SIZE="10G"' >> ~/.bashrc
echo 'export SCCACHE_DIR="$HOME/.cache/sccache"' >> ~/.bashrc
```

## Useful Monitoring Scripts

### Script 1: Build with Stats Comparison
```bash
#!/bin/bash
# before_after_build.sh

echo "=== Before Build ==="
sccache -s

echo -e "\n=== Building... ===\n"
cargo build

echo -e "\n=== After Build ==="
sccache -s
```

### Script 2: Cache Efficiency Report
```bash
#!/bin/bash
# cache_report.sh

stats=$(sccache -s)

executed=$(echo "$stats" | grep "Compile requests executed" | awk '{print $4}')
hits=$(echo "$stats" | grep "Cache hits" | grep -v "C/C++" | grep -v "Rust" | head -1 | awk '{print $3}')
misses=$(echo "$stats" | grep "Cache misses" | awk '{print $3}')
size=$(echo "$stats" | grep "^Cache size" | awk '{print $3, $4}')

if [ "$executed" -gt 0 ]; then
    hit_rate=$(echo "scale=1; ($hits / $executed) * 100" | bc)
    echo "╔════════════════════════════════════╗"
    echo "║     sccache Performance Report     ║"
    echo "╠════════════════════════════════════╣"
    echo "║ Hit Rate:        $hit_rate%          ║"
    echo "║ Total Compiled:  $executed              ║"
    echo "║ Cache Hits:      $hits              ║"
    echo "║ Cache Misses:    $misses               ║"
    echo "║ Cache Size:      $size       ║"
    echo "╚════════════════════════════════════╝"
else
    echo "No compilations yet"
fi
```

### Script 3: Monitor During Build
```bash
#!/bin/bash
# monitor_build.sh

# Start monitoring in background
(
    while true; do
        clear
        echo "=== sccache Live Monitor (Ctrl+C to stop) ==="
        echo ""
        sccache -s | grep -E "Compile requests|Cache hits|Cache misses|Cache size"
        echo ""
        echo "Last updated: $(date '+%H:%M:%S')"
        sleep 2
    done
) &
MONITOR_PID=$!

# Run your build
cargo build "$@"

# Stop monitoring
kill $MONITOR_PID 2>/dev/null
```

## Integration with Cargo

### Build with Stats Output
```bash
# Build and show stats after
cargo build && sccache -s

# Clean build to test cache effectiveness
cargo clean && sccache -z && cargo build && sccache -s

# Build specific package and check stats
cargo build -p llm && sccache -s
```

### Compare Clean vs Cached Build
```bash
# First build (populates cache)
cargo clean
sccache -z
time cargo build

# Second build (uses cache)
cargo clean
time cargo build

# Show stats
sccache -s
```

## Environment Variables Reference

```bash
# Essential
export RUSTC_WRAPPER=sccache           # Enable sccache for Rust
export SCCACHE_CACHE_SIZE="10G"        # Max cache size
export SCCACHE_DIR="$HOME/.cache/sccache"  # Cache location

# Advanced
export SCCACHE_LOG=info                # Enable logging
export SCCACHE_ERROR_LOG=/tmp/sccache_err.log  # Error log location
export SCCACHE_IDLE_TIMEOUT=300        # Server idle timeout (seconds)

# Distributed compilation (advanced)
export SCCACHE_REDIS=redis://localhost:6379  # Redis for distributed cache
```

## Troubleshooting Commands

### Check if sccache is Working
```bash
# Should show sccache in the output
echo $RUSTC_WRAPPER

# Should return: sccache
which $RUSTC_WRAPPER

# Verify it's being used
ps aux | grep sccache
```

### Debug Mode
```bash
# Enable verbose logging
SCCACHE_LOG=debug cargo build

# Check logs
tail -f ~/.cache/sccache/*.log
```

### Verify Cache is Being Used
```bash
# Build twice and compare times
cargo clean && time cargo build  # Should be slow
cargo clean && time cargo build  # Should be MUCH faster
```

## Performance Metrics to Watch

**Good cache performance:**
- Hit rate > 80% on repeated builds
- Cache read hit time < 1 second
- Significant time savings on rebuilds

**Poor cache performance:**
- Hit rate < 50%
- Many cache misses
- May need to increase `SCCACHE_CACHE_SIZE`

## Daily Workflow

```bash
# Morning: Clear old stats
sccache -z

# During development: Check stats periodically
sccache -s

# End of day: Review cache efficiency
sccache -s | grep "Cache hits"

# Weekly: Clean cache if too large
du -sh ~/.cache/sccache
# If > 10GB, consider clearing old entries
```

## Aliases for .bashrc/.zshrc

```bash
# Add these to your shell config
alias sc='sccache -s'                    # Quick stats
alias scz='sccache -z'                   # Zero stats
alias scs='sccache --stop-server'        # Stop server
alias scr='sccache -z && sccache --stop-server'  # Full reset
alias scw='watch -n 2 "sccache -s"'      # Watch stats
```