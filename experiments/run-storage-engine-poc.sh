#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
stamp=$(date -u +%Y%m%dT%H%M%SZ)
result_dir="$project_dir/experiments/results/storage-engine-$stamp"
binary="$project_dir/target/release/examples/storage_engine_poc"
work_dir=$(mktemp -d /private/tmp/clipboard-history-poc.XXXXXX)
base_db="$work_dir/base-100k.sqlite"

cleanup() {
  rm -rf "$work_dir"
}
trap cleanup EXIT INT TERM

mkdir -p "$result_dir"
cargo build --release --manifest-path "$project_dir/Cargo.toml" -p clipboard-store --example storage_engine_poc

"$binary" seed "$base_db" 100000 > "$result_dir/seed.txt"

for cache_kib in 1024 4096 16384; do
  for warm_up in 0 1; do
    "$binary" cold "$base_db" "$cache_kib" "$warm_up" \
      > "$result_dir/cold-cache-${cache_kib}-warm-${warm_up}.txt"
  done
done

cp "$base_db" "$work_dir/wal.sqlite"
"$binary" wal "$work_dir/wal.sqlite" 10000 > "$result_dir/wal.txt"

for batch_size in 100 250 500 1000; do
  cp "$base_db" "$work_dir/prune-${batch_size}.sqlite"
  "$binary" prune "$work_dir/prune-${batch_size}.sqlite" 10000 "$batch_size" \
    > "$result_dir/prune-${batch_size}.txt"
done

"$binary" overflow "$work_dir/overflow" > "$result_dir/overflow.txt"
"$binary" payload "$work_dir/payload" > "$result_dir/payload.txt"

: > "$result_dir/crash.txt"
for crash_stage in staged_temp after_rename_before_row after_row_commit after_delete_commit after_file_delete; do
  case_dir="$work_dir/crash-$crash_stage"
  set +e
  "$binary" crash-prepare "$case_dir" "$crash_stage"
  crash_status=$?
  set -e
  if [ "$crash_status" -ne 86 ]; then
    printf 'unexpected crash status stage=%s status=%s\n' "$crash_stage" "$crash_status" >&2
    exit 1
  fi
  "$binary" crash-verify "$case_dir" "$crash_stage" >> "$result_dir/crash.txt"
done

printf 'results=%s\n' "$result_dir"
