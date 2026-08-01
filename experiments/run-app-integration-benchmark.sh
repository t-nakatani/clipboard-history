#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
project_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
generated_dir="$project_dir/app/macos/Generated"
build_dir="$script_dir/app-integration-benchmark/build"
binary="$build_dir/ClipboardHistoryAppIntegrationBenchmark"
module_cache_dir="$build_dir/module-cache"
developer_dir=$(/usr/bin/xcode-select -p)
compile_sdk=$(/usr/bin/xcrun --sdk macosx --show-sdk-path)
link_sdk="$compile_sdk"
swift_core_stub=""
compiler_version=$(/usr/bin/swiftc --version | head -n 1)
sdk_interface="$link_sdk/usr/lib/swift/Swift.swiftmodule/arm64e-apple-macos.swiftinterface"
sdk_compiler_version=$(sed -n '2p' "$sdk_interface" 2>/dev/null || true)

if [ "$developer_dir" = "/Library/Developer/CommandLineTools" ] && \
  [ "${sdk_compiler_version#*"$compiler_version"}" = "$sdk_compiler_version" ] && \
  [ -d "$developer_dir/SDKs/MacOSX13.3.sdk" ] && \
  [ -f "$link_sdk/usr/lib/swift/libswiftCore.tbd" ]; then
  compile_sdk="$developer_dir/SDKs/MacOSX13.3.sdk"
  swift_core_stub="$link_sdk/usr/lib/swift/libswiftCore.tbd"
fi
rows=${APP_INTEGRATION_BENCHMARK_ROWS:-100000}
scroll_pages=${APP_INTEGRATION_BENCHMARK_SCROLL_PAGES:-2000}
menu_runs=${APP_INTEGRATION_BENCHMARK_MENU_RUNS:-5}
output_dir=${APP_INTEGRATION_BENCHMARK_OUTPUT_DIR:-"$project_dir/experiments/results/app-integration-benchmark-$(date -u +%Y%m%dT%H%M%SZ)"}

mkdir -p "$generated_dir" "$build_dir" "$output_dir"

cargo build --release -p clipboard-ffi
cargo build -p clipboard-ffi --features bindgen-cli --bin clipboard-uniffi-bindgen

rm -f \
  "$generated_dir/clipboard_ffi.swift" \
  "$generated_dir/clipboard_ffiFFI.h" \
  "$generated_dir/clipboard_ffiFFI.modulemap"
"$project_dir/target/debug/clipboard-uniffi-bindgen" generate \
  --library "$project_dir/target/release/libclipboard_ffi.dylib" \
  --language swift \
  --metadata-no-deps \
  --out-dir "$generated_dir"

/usr/bin/swiftc \
  "$project_dir/app/macos/Sources/HistoryPanelConfiguration.swift" \
  "$project_dir/app/macos/Sources/HistoryPageWindow.swift" \
  "$project_dir/app/macos/Sources/HistoryPanel.swift" \
  "$project_dir/app/macos/Sources/ImagePreviewGenerator.swift" \
  "$project_dir/app/macos/Sources/HistoryStoreClient.swift" \
  "$script_dir/app-integration-benchmark/main.swift" \
  "$generated_dir/clipboard_ffi.swift" \
  -sdk "$compile_sdk" \
  -module-cache-path "$module_cache_dir" \
  -I "$generated_dir" \
  -Xcc "-fmodule-map-file=$generated_dir/clipboard_ffiFFI.modulemap" \
  -L "$project_dir/target/release" \
  -lclipboard_ffi \
  -framework AppKit \
  -framework ImageIO \
  ${swift_core_stub:+"$swift_core_stub"} \
  -Xlinker -rpath \
  -Xlinker "$project_dir/target/release" \
  -o "$binary"

run_scenario() {
  scenario=$1
  scenario_dir=$(mktemp -d "${TMPDIR:-/tmp}/clipboard-history-app-benchmark-${scenario}.XXXXXX")
  trap 'rm -rf "$scenario_dir"' EXIT HUP INT TERM

  "$binary" \
    --mode seed \
    --scenario "$scenario" \
    --root "$scenario_dir" \
    --rows "$rows" \
    >"$output_dir/$scenario-seed.txt" 2>&1
  "$binary" \
    --mode measure \
    --scenario "$scenario" \
    --root "$scenario_dir" \
    --rows "$rows" \
    --scroll-pages "$scroll_pages" \
    --menu-runs "$menu_runs" \
    >"$output_dir/$scenario-measure.txt" 2>&1

  rm -rf "$scenario_dir"
  trap - EXIT HUP INT TERM
}

for scenario in text-only mixed-images huge-payload; do
  run_scenario "$scenario"
done

printf 'benchmark_results=%s\n' "$output_dir"
