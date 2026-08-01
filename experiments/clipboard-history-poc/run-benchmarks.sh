#!/bin/sh
set -eu

experiment_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
binary="$experiment_dir/target/release/clipboard-history-poc"
output_dir="$experiment_dir/results"

mkdir -p "$output_dir"
cargo build --release --manifest-path "$experiment_dir/Cargo.toml"

for count in 1000 10000 100000; do
  db_path="$output_dir/history-$count.sqlite"
  result_path="$output_dir/result-$count.txt"
  "$binary" "$count" "$db_path" > "$result_path"
  printf 'completed count=%s result=%s\n' "$count" "$result_path"
done
