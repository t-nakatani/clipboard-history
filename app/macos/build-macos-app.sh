#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
project_dir=$(CDPATH= cd -- "$script_dir/../.." && pwd)
generated_dir="$script_dir/Generated"
build_dir="$script_dir/build"
app_dir="$build_dir/ClipboardHistory.app"
module_cache_dir="$build_dir/module-cache"
developer_dir=$(/usr/bin/xcode-select -p)
compile_sdk=$(/usr/bin/xcrun --sdk macosx --show-sdk-path)
link_sdk="$compile_sdk"
swift_core_stub=""
compiler_version=$(/usr/bin/swiftc --version | head -n 1)
sdk_interface="$link_sdk/usr/lib/swift/Swift.swiftmodule/arm64e-apple-macos.swiftinterface"
sdk_compiler_version=$(sed -n '2p' "$sdk_interface" 2>/dev/null || true)

# Some standalone CLT 15.0 installations pair swiftlang-5.9.0.128 with a
# 5.9.0.123 SDK. Use the compatible 13.3 interfaces and the current SDK's
# linker stub only for that known CLT-only layout. Full Xcode uses its SDK.
if [ "$developer_dir" = "/Library/Developer/CommandLineTools" ] && \
   [ "${sdk_compiler_version#*"$compiler_version"}" = "$sdk_compiler_version" ] && \
   [ -d "$developer_dir/SDKs/MacOSX13.3.sdk" ] && \
   [ -f "$link_sdk/usr/lib/swift/libswiftCore.tbd" ]; then
    compile_sdk="$developer_dir/SDKs/MacOSX13.3.sdk"
    swift_core_stub="$link_sdk/usr/lib/swift/libswiftCore.tbd"
fi

cd "$project_dir"
cargo build --release -p clipboard-ffi
cargo build -p clipboard-ffi --features bindgen-cli --bin clipboard-uniffi-bindgen

rm -f "$generated_dir/clipboard_ffi.swift" "$generated_dir/clipboard_ffiFFI.h" "$generated_dir/clipboard_ffiFFI.modulemap"
"$project_dir/target/debug/clipboard-uniffi-bindgen" generate \
    --library "$project_dir/target/release/libclipboard_ffi.dylib" \
    --language swift \
    --metadata-no-deps \
    --out-dir "$generated_dir"

mkdir -p "$app_dir/Contents/MacOS" "$app_dir/Contents/Frameworks" "$app_dir/Contents/Resources" "$module_cache_dir"
cp "$script_dir/Info.plist" "$app_dir/Contents/Info.plist"
cp "$project_dir/target/release/libclipboard_ffi.dylib" "$app_dir/Contents/Frameworks/"
rust_dylib_id=$(/usr/bin/otool -D "$project_dir/target/release/libclipboard_ffi.dylib" | tail -n 1)
/usr/bin/install_name_tool -id @rpath/libclipboard_ffi.dylib "$app_dir/Contents/Frameworks/libclipboard_ffi.dylib"

/usr/bin/swiftc \
    "$script_dir/Sources/main.swift" \
    "$script_dir/Sources/AppDelegate.swift" \
    "$script_dir/Sources/HistoryPanel.swift" \
    "$script_dir/Sources/HistoryPanelConfiguration.swift" \
    "$script_dir/Sources/HistoryPageWindow.swift" \
    "$script_dir/Sources/HistoryStoreClient.swift" \
    "$script_dir/Sources/ImagePreviewGenerator.swift" \
    "$script_dir/Sources/PasteboardMonitor.swift" \
    "$script_dir/Sources/PasteboardWriter.swift" \
    "$generated_dir/clipboard_ffi.swift" \
    -sdk "$compile_sdk" \
    -module-cache-path "$module_cache_dir" \
    -I "$generated_dir" \
    -Xcc "-fmodule-map-file=$generated_dir/clipboard_ffiFFI.modulemap" \
    -L "$project_dir/target/release" \
    -lclipboard_ffi \
    ${swift_core_stub:+"$swift_core_stub"} \
    -framework AppKit \
    -framework ImageIO \
    -Xlinker -rpath \
    -Xlinker @executable_path/../Frameworks \
    -o "$app_dir/Contents/MacOS/ClipboardHistory"

/usr/bin/install_name_tool \
    -change "$rust_dylib_id" @rpath/libclipboard_ffi.dylib \
    "$app_dir/Contents/MacOS/ClipboardHistory"

"$app_dir/Contents/MacOS/ClipboardHistory" --self-test
echo "Built $app_dir"
