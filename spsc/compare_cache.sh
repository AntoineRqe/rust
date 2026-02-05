#!/bin/bash

CORES="1,3"

echo "╔════════════════════════════════════════════════════════╗"
echo "║     Cache Padding Impact Analysis                      ║"
echo "╚════════════════════════════════════════════════════════╝"
echo ""

# Build both versions
echo "Building binaries..."
cargo build --profile profiling --example profile_aligned --features cache-padding 2>/dev/null
cargo build --profile profiling --example profile_unaligned --no-default-features 2>/dev/null

echo ""
echo "================================"
echo "WITHOUT Cache Padding (unaligned)"
echo "================================"

unaligned_output=$(taskset -c $CORES perf stat \
    -e cache-misses,L1-dcache-load-misses,LLC-load-misses \
    target/profiling/examples/profile_unaligned 2>&1)

echo "$unaligned_output"

echo ""
echo "================================"
echo "WITH Cache Padding (aligned)"
echo "================================"

aligned_output=$(taskset -c $CORES perf stat \
    -e cache-misses,L1-dcache-load-misses,LLC-load-misses \
    target/profiling/examples/profile_aligned 2>&1)

echo "$aligned_output"

# Extract time - get first field before "seconds", handle both . and , as decimal separator
aligned_time=$(echo "$aligned_output" | grep "seconds time elapsed" | awk '{print $1}' | tr ',' '.')
unaligned_time=$(echo "$unaligned_output" | grep "seconds time elapsed" | awk '{print $1}' | tr ',' '.')

# Extract metrics - remove ALL whitespace
aligned_cache=$(echo "$aligned_output" | grep "cpu_core/cache-misses" | awk '{gsub(/[[:space:]]/, "", $1); print $1}')
aligned_l1=$(echo "$aligned_output" | grep "cpu_core/L1-dcache-load-misses" | awk '{gsub(/[[:space:]]/, "", $1); print $1}')
aligned_llc=$(echo "$aligned_output" | grep "cpu_core/LLC-load-misses" | awk '{gsub(/[[:space:]]/, "", $1); print $1}')

unaligned_cache=$(echo "$unaligned_output" | grep "cpu_core/cache-misses" | awk '{gsub(/[[:space:]]/, "", $1); print $1}')
unaligned_l1=$(echo "$unaligned_output" | grep "cpu_core/L1-dcache-load-misses" | awk '{gsub(/[[:space:]]/, "", $1); print $1}')
unaligned_llc=$(echo "$unaligned_output" | grep "cpu_core/LLC-load-misses" | awk '{gsub(/[[:space:]]/, "", $1); print $1}')

if [ -n "$aligned_time" ] && [ -n "$unaligned_time" ] && [ -n "$aligned_cache" ] && [ -n "$unaligned_cache" ]; then
    # Calculate differences (negative = without padding is better, positive = with padding is better)
    time_diff=$(LC_NUMERIC=C awk -v a="$aligned_time" -v b="$unaligned_time" 'BEGIN {printf "%.2f", ((b - a) / a) * 100}')
    cache_diff=$(LC_NUMERIC=C awk -v a="$aligned_cache" -v b="$unaligned_cache" 'BEGIN {printf "%.2f", ((b - a) / a) * 100}')
    l1_diff=$(LC_NUMERIC=C awk -v a="$aligned_l1" -v b="$unaligned_l1" 'BEGIN {printf "%.2f", ((b - a) / a) * 100}')
    llc_diff=$(LC_NUMERIC=C awk -v a="$aligned_llc" -v b="$unaligned_llc" 'BEGIN {printf "%.2f", ((b - a) / a) * 100}')
    
    # Calculate speedup/slowdown
    speedup=$(LC_NUMERIC=C awk -v a="$aligned_time" -v b="$unaligned_time" 'BEGIN {printf "%.2f", b / a}')

    echo ""
    echo "╔════════════════════════════════════════════════════════════════╗"
    echo "║                    Results Summary                             ║"
    echo "╠════════════════════════════════════════════════════════════════╣"
    echo "║ Metric              │ With Padding │ Without      │ Difference  ║"
    echo "╠═════════════════════╪══════════════╪══════════════╪═════════════╣"
    printf "║ Time (s)            │ %12s │ %12s │ %10s%% ║\n" "$aligned_time" "$unaligned_time" "$time_diff"
    printf "║ Cache Misses        │ %12s │ %12s │ %10s%% ║\n" "$aligned_cache" "$unaligned_cache" "$cache_diff"
    printf "║ L1 Cache Misses     │ %12s │ %12s │ %10s%% ║\n" "$aligned_l1" "$unaligned_l1" "$l1_diff"
    printf "║ LLC Misses          │ %12s │ %12s │ %10s%% ║\n" "$aligned_llc" "$unaligned_llc" "$llc_diff"
    echo "╚═════════════════════╧══════════════╧══════════════╧═════════════╝"
    echo ""
    
    # Correct interpretation
    if (( $(echo "$time_diff < 0" | bc -l) )); then
        # Negative = without padding was faster
        echo "⚠️  UNEXPECTED RESULT:"
        echo "  WITHOUT padding was ${time_diff#-}% faster (${speedup}x)"
        echo "  This suggests padding may be HURTING performance"
        echo ""
        echo "Possible reasons:"
        echo "  • Cache line alignment may not be the bottleneck"
        echo "  • Larger memory footprint from padding causes more cache pressure"
        echo "  • Your workload may not have significant false sharing"
        echo "  • System variance - try running multiple times"
    else
        # Positive = with padding was faster
        echo "✅ EXPECTED RESULT:"
        echo "  WITH padding was ${time_diff}% faster (${speedup}x)"
        echo "  Cache padding is working as intended!"
        echo ""
        echo "Benefits:"
        echo "  • Reduces L1 cache misses by ${l1_diff#-}%"
        echo "  • Reduces overall cache misses by ${cache_diff#-}%"
        echo "  • Eliminates false sharing between producer/consumer"
    fi
else
    echo "❌ Error: Could not extract metrics"
fi