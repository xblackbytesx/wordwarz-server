#!/usr/bin/env bash
# Downloads the pinned word lists, checks them, and rebuilds lexicons/*.fst
# and the tile sets in crates/core/src/tilesets.rs. Run via `make lexicons`.
# Pins and licences: lexicons/SOURCES.md. A changed tile set is a new rules
# version (AGENTS.md decision 3), so review the tilesets.rs diff before keeping it.
set -euo pipefail
cd "$(dirname "$0")/.."

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

fetch() { # url sha256 out
  curl -fsSL "$1" -o "$3"
  echo "$2  $3" | sha256sum -c --quiet -
}

fetch "https://raw.githubusercontent.com/OpenTaal/opentaal-wordlist/b250510dda431785f962019167d1415198ff3905/wordlist.txt" \
  12e5fb5e3c73840b583b30016926d1f63a75e9bf1652a3a6634b2ba7c49ad7be "$work/nl.txt"
fetch "https://norvig.com/ngrams/enable1.txt" \
  61ba1392a5b6199dd161fae7b483cd262fb8a88c0d49bd2c9ecb6753e755697f "$work/en.txt"

cargo build --release -q -p wordwarz-lexicon
tool="${CARGO_TARGET_DIR:-target}/release/wordwarz-lexicon"
"$tool" build "$work/nl.txt" lexicons/nl.fst
"$tool" build "$work/en.txt" lexicons/en.fst
"$tool" tiles lexicons crates/core/src/tilesets.rs
