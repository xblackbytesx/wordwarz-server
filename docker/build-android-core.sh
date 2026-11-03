#!/usr/bin/env bash
# Builds what the Android app consumes from this repo, into dist/:
#   dist/android/jniLibs/<abi>/libwordwarz_ffi.so   stripped release libs (cargo-ndk)
#   dist/android/kotlin/uniffi/wordwarz_ffi/*.kt    generated Kotlin bindings
#   dist/host/libwordwarz_ffi.so                    host lib for the app's JVM tests
#   dist/assets/lexicons/{nl,en}.fst + licence      the word lists, as app assets
# Bindings come from an UNSTRIPPED debug build: UniFFI's library mode reads
# metadata symbols that the release profile's `strip = true` removes.
set -euo pipefail
cd "$(dirname "$0")/.."

API="${ANDROID_API:-30}"   # = the app's minSdk
T="${CARGO_TARGET_DIR:-target}"
rm -rf dist/android dist/host dist/assets
mkdir -p dist/android/kotlin dist/host dist/assets/lexicons

cargo build -p wordwarz-ffi
cargo run -q -p wordwarz-ffi --features cli --bin uniffi-bindgen -- \
  generate --library "$T/debug/libwordwarz_ffi.so" --language kotlin --no-format --out-dir dist/android/kotlin

cargo build -p wordwarz-ffi --release
cp "$T/release/libwordwarz_ffi.so" dist/host/

if [ "${HOST_ONLY:-0}" != "1" ]; then
  cargo ndk --platform "$API" -t arm64-v8a -t x86_64 -o dist/android/jniLibs build -p wordwarz-ffi --release
fi

cp lexicons/nl.fst lexicons/en.fst lexicons/LICENSE-opentaal.txt lexicons/SOURCES.md dist/assets/lexicons/

# Hand ownership back to the host user when run as root in the container.
owner="$(stat -c '%u:%g' . 2>/dev/null || true)"
[ -n "$owner" ] && chown -R "$owner" dist 2>/dev/null || true
find dist -type f | sort
