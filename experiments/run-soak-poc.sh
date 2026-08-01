#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
stamp=$(date -u +%Y%m%dT%H%M%SZ)
result_dir="$project_dir/experiments/results/soak-$stamp"
binary="$project_dir/target/release/examples/storage_engine_poc"
work_dir=$(mktemp -d /private/tmp/clipboard-soak-poc.XXXXXX)

cleanup() {
  rm -rf "$work_dir"
}
trap cleanup EXIT INT TERM

mkdir -p "$result_dir"
cargo build --release --manifest-path "$project_dir/Cargo.toml" -p clipboard-store --example storage_engine_poc

"$binary" soak "$work_dir/soak" 250000 10000 2500 > "$result_dir/soak.txt"

for file_count in 1000 10000 50000 100000; do
  "$binary" orphan-scan "$work_dir/orphan-$file_count" "$file_count" \
    > "$result_dir/orphan-$file_count.txt"
done

printf 'results=%s\n' "$result_dir"
