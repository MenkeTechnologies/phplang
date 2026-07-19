#!/usr/bin/env bash
# Fetch the Symfony polyfill source used by the stdlib byte-parity harness
# (src/bin/parity_stdlib.rs) into a gitignored directory.
#
# The Symfony polyfills are MIT-licensed pure-PHP reimplementations of PHP
# standard-library functions. To keep the repo self-contained and follow the
# same convention as the Drupal harness, the source is fetched on demand into
# third_party/php-polyfill/ (which .gitignore excludes) rather than vendored.
# The harness extracts individual function bodies and diffs phplang's output
# against the reference `php` byte for byte.
set -euo pipefail

dest="$(cd "$(dirname "$0")/.." && pwd)/third_party/php-polyfill"
mkdir -p "$dest"

# file = raw URL under a polyfill package's 1.x branch.
fetch() {
    local name="$1" url="$2"
    echo "fetching $name ..."
    curl -fsSL --max-time 30 -o "$dest/$name" "$url"
}

fetch Php80.php "https://raw.githubusercontent.com/symfony/polyfill-php80/1.x/Php80.php"

echo "Symfony polyfill source fetched to $dest"
