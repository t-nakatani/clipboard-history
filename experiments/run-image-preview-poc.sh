#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
project_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
timestamp=$(date -u +%Y%m%dT%H%M%SZ)
result_dir="$script_dir/results/image-preview-$timestamp"
binary="$result_dir/image-preview-poc"
iterations=${IMAGE_PREVIEW_ITERATIONS:-200}
storage_rows=${IMAGE_PREVIEW_STORAGE_ROWS:-100000}

mkdir -p "$result_dir"

/usr/bin/swiftc \
    "$project_dir/app/macos/Sources/ImagePreviewGenerator.swift" \
    "$script_dir/image-preview-poc/main.swift" \
    -framework AppKit \
    -framework ImageIO \
    -o "$binary"

"$binary" "$result_dir" "$iterations" > "$result_dir/generation.txt"
cat "$result_dir/generation.txt"
cargo run --release -p clipboard-store --example image_preview_storage_poc -- \
    "$result_dir" "$storage_rows" > "$result_dir/storage.txt"
cat "$result_dir/storage.txt"

rm -f "$binary"
echo "Results: $result_dir"
