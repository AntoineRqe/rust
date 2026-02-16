#!/bin/bash

CORES="0,7"

echo "╔════════════════════════════════════════════════════════╗"
echo "║     Cache Padding Impact Analysis (AMD Zen 4)         ║"
echo "╚════════════════════════════════════════════════════════╝"
echo ""

EVENTS="cycles,instructions,cache-misses,L1-dcache-loads,L1-dcache-load-misses,L1-icache-loads,L1-icache-load-misses,l2_cache_req_stat.ic_dc_miss_in_l2"

# Build both versions
echo "Building binaries..."
cargo build --profile profiling --example profile_aligned --features cache-padding 2>/dev/null
cargo build --profile profiling --example profile_lock --no-default-features 2>/dev/null

echo ""
echo "================================"
echo "WITHOUT padding (unaligned)"
echo "================================"

unaligned_output=$(taskset -c $CORES perf stat \
    -e $EVENTS \
    target/profiling/examples/profile_lock 2>&1)

echo "$unaligned_output"

echo ""
echo "================================"
echo "WITH Cache Padding (aligned)"
echo "================================"

aligned_output=$(taskset -c $CORES perf stat \
    -e $EVENTS \
    target/profiling/examples/profile_aligned 2>&1)

echo "$aligned_output"

# Extract time
aligned_time=$(echo "$aligned_output" | grep "seconds time elapsed" | awk '{print $1}' | tr ',' '.')
unaligned_time=$(echo "$unaligned_output" | grep "seconds time elapsed" | awk '{print $1}' | tr ',' '.')

# Generic metric extraction helper
extract_metric() {
    echo "$1" | grep "$2" | awk '{gsub(/[[:space:]]/, "", $1); print $1}'
}

aligned_cache=$(extract_metric "$aligned_output" "cache-misses")
unaligned_cache=$(extract_metric "$unaligned_output" "cache-misses")

aligned_l1=$(extract_metric "$aligned_output" "L1-dcache-load-misses")
unaligned_l1=$(extract_metric "$unaligned_output" "L1-dcache-load-misses")

aligned_l2=$(extract_metric "$aligned_output" "l2_cache_req_stat.ic_dc_miss_in_l2")
unaligned_l2=$(extract_metric "$unaligned_output" "l2_cache_req_stat.ic_dc_miss_in_l2")

echo ""
echo "Raw extracted values:"
echo "aligned_time=$aligned_time"
echo "unaligned_time=$unaligned_time"
echo "aligned_cache=$aligned_cache"
echo "unaligned_cache=$unaligned_cache"
echo "aligned_l1=$aligned_l1"
echo "unaligned_l1=$unaligned_l1"
echo "aligned_l2=$aligned_l2"
echo "unaligned_l2=$unaligned_l2"

if [ -n "$aligned_time" ] && [ -n "$unaligned_time" ]; then
    time_diff=$(LC_NUMERIC=C awk -v a="$aligned_time" -v b="$unaligned_time" 'BEGIN {printf "%.2f", ((b - a) / a) * 100}')
    cache_diff=$(LC_NUMERIC=C awk -v a="$aligned_cache" -v b="$unaligned_cache" 'BEGIN {printf "%.2f", ((b - a) / a) * 100}')
    l1_diff=$(LC_NUMERIC=C awk -v a="$aligned_l1" -v b="$unaligned_l1" 'BEGIN {printf "%.2f", ((b - a) / a) * 100}')
    l2_diff=$(LC_NUMERIC=C awk -v a="$aligned_l2" -v b="$unaligned_l2" 'BEGIN {printf "%.2f", ((b - a) / a) * 100}')

    speedup=$(LC_NUMERIC=C awk -v a="$aligned_time" -v b="$unaligned_time" 'BEGIN {printf "%.2f", b / a}')

    echo ""
    echo "╔════════════════════════════════════════════════════════════════╗"
    echo "║                 Cache Padding Impact Summary                   ║"
    echo "╠═════════════════════╪══════════════╪══════════════╪════════════╣"
    echo "║ Metric              │ With Padding │ Without      │ Difference ║"
    echo "╠═════════════════════╪══════════════╪══════════════╪════════════╣"
    printf "║ Time (s)            │ %12s │ %12s │ %10s%% ║\n" "$aligned_time" "$unaligned_time" "$time_diff"
    printf "║ Cache Misses        │ %12s │ %12s │ %10s%% ║\n" "$aligned_cache" "$unaligned_cache" "$cache_diff"
    printf "║ L1 Cache Misses     │ %12s │ %12s │ %10s%% ║\n" "$aligned_l1" "$unaligned_l1" "$l1_diff"
    printf "║ L2 Cache Misses     │ %12s │ %12s │ %10s%% ║\n" "$aligned_l2" "$unaligned_l2" "$l2_diff"
    echo "╚═════════════════════╧══════════════╧══════════════╧════════════╝"
    echo ""

    if (( $(echo "$time_diff < 0" | bc -l) )); then
        echo "⚠️  WITHOUT padding was faster by ${time_diff#-}% (${speedup}x)"
        echo "   → Indicates low false sharing or padding overhead"
    else
        echo "✅ WITH padding was faster by ${time_diff}% (${speedup}x)"
        echo "   → Strong evidence of cache-line contention removal"
        echo "   → L1 miss reduction: ${l1_diff#-}%"
        echo "   → L2 miss reduction: ${l2_diff#-}%"
    fi
else
    echo "❌ Error: Could not extract metrics"
fi
