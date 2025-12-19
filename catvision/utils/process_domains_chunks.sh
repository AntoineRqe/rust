#!/usr/bin/env bash

# Usage: ./run_all_domain_files.sh /path/to/domains_folder

set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "Usage: $0 domains_folder"
    exit 1
fi

DOMAIN_DIR="$1"

if [[ ! -d "$DOMAIN_DIR" ]]; then
    echo "Directory not found: $DOMAIN_DIR"
    exit 1
fi

echo "Running cargo command for all files in: $DOMAIN_DIR"

# Loop through all files (sorted)
for file in "$(realpath "$DOMAIN_DIR")"/*; do
    [[ -f "$file" ]] || continue

    echo "----------------------------------------"
    echo "Processing: $file"
    echo "----------------------------------------"

    cargo run --release -- --input "$file" --config configs/config-prod.json
done

echo "All files processed successfully."
