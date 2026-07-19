#!/usr/bin/env bash
# Fetch the Drupal 7 core include files used by the byte-parity harness
# (src/bin/parity_drupal.rs) into a gitignored directory.
#
# Drupal is GPL-2.0-or-later; phplang is MIT. Its source is therefore NEVER
# committed to this repo — it is fetched on demand into third_party/drupal/,
# which .gitignore excludes. The harness extracts individual procedural
# functions from these files at runtime and diffs phplang's output against the
# reference `php` byte for byte.
set -euo pipefail

dest="$(cd "$(dirname "$0")/.." && pwd)/third_party/drupal/includes"
base="https://raw.githubusercontent.com/drupal/drupal/7.x/includes"

mkdir -p "$dest"
for f in bootstrap.inc common.inc; do
    echo "fetching $f ..."
    curl -fsSL --max-time 30 -o "$dest/$f" "$base/$f"
done
echo "Drupal 7 includes fetched to $dest"
