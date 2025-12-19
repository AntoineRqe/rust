#!/usr/bin/env bash

# Usage: ./split_domains_csv.sh /path/to/domains.txt

set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "Usage: $0 input_file"
    exit 1
fi

INPUT_FILE="$1"
CHUNK_SIZE=100000
TMP_PREFIX="tmp_chunk_"
OUT_PREFIX="domains_chunk_"

# Determine the directory of the input file
BASE_DIR="$(dirname "$INPUT_FILE")"
CHUNK_DIR="${BASE_DIR}/chunks"

# Create output directory
mkdir -p "$CHUNK_DIR"

echo "Chunks will be stored in: $CHUNK_DIR"

# Split into temporary chunks (no headers yet)
split -l "$CHUNK_SIZE" -d -a 4 "$INPUT_FILE" "${CHUNK_DIR}/${TMP_PREFIX}"

# Now convert temporary chunks into CSV files with header
for file in "${CHUNK_DIR}/${TMP_PREFIX}"*; do
    index="${file##*${TMP_PREFIX}}"
    outfile="${CHUNK_DIR}/${OUT_PREFIX}${index}.csv"

    # Write header
    echo "domain" > "$outfile"

    # Append chunk data
    cat "$file" >> "$outfile"

    echo "Created $outfile"
done

# Remove temporary files
rm "${CHUNK_DIR}/${TMP_PREFIX}"*

echo "All CSV chunks created successfully in: $CHUNK_DIR"
