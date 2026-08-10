# Changelog

Behavioural changes to the engine, newest first. Everything here is measured
against the reference implementation; see [BUGS.md](BUGS.md) for divergences
that are known and **not** fixed.

## Reference oracle

Every expectation in this file and in `tests/` comes from a recorded run of:

| | |
|---|---|
| binary | `/opt/homebrew/bin/php` → `/opt/homebrew/Cellar/php/8.5.9/bin/php` |
| version | `PHP 8.5.9 (cli) (built: Jul 28 2026 13:06:52) (NTS)`, Zend Engine v4.5.9 |
| entry point | `php -r` unless stated — script name `Command line code`. `php FILE`, `php -f` and stdin name themselves differently and are pinned separately in `tests/cli_entry_points.rs` |
| php.ini | `/opt/homebrew/etc/php/8.5/php.ini` (no scanned `.d` files) |
| `error_reporting` | `30719` (`E_ALL`; `E_STRICT` is gone in 8.4+) |
| `display_errors` | `1` (STDOUT) |
| `log_errors` | `1` — so every diagnostic appears TWICE, once on stdout and once on stderr with a `PHP ` prefix |
| `date.timezone` | commented out in php.ini; effective value `UTC` |
| `precision` / `serialize_precision` | `14` / `-1` |
| environment | `LC_ALL=C TZ=UTC` on every probe |

Where a fix is a port, the C source it was ported from is named in the Rust doc
comment above it.

---

## Round 7 — degenerate input, vacuous tests, and error shape

### Panics, hangs and aborts turned into PHP behaviour

Twenty-one inputs that stopped the process are now answered the way the
reference answers them. A Rust panic, a stack overflow, or a scaffold-level
`php: …` failure is a parity divergence even when the happy path matches,
because PHP code cannot `catch` any of them.

**Integer overflow — widen or saturate, never wrap**

- `$x++` / `$x--` off either end of the int range now produce a **float**
  (`PHP_INT_MAX + 1` is `9.223372036854776E+18`), for the int, pre-increment and
  numeric-string forms alike. Previously `attempt to add with overflow`.
- The array next-free index now saturates at `PHP_INT_MAX` across all six
  writers that maintained it (`$a[k] =`, an array literal, a numeric-string key,
  `$a[] =`, `&$a[k]`, `$a[k] = &$x`).
- An append with nowhere to go — the key `PHP_INT_MAX` already taken — is the
  catchable `Error: Cannot add element to the array as the next element is
  already occupied`, from `$a[] =`, `$a[] = &$x` and `array_push` alike. Ported
  from `_zend_hash_index_add_or_update_i`: an append is an ADD at
  `nNextFreeElement`, and an ADD onto an existing key fails.

**`range()` — re-ported in full from `PHP_FUNCTION(range)`**

23 of 34 probed forms diverged, including one infinite loop (`range(0, 10,
NAN)`) and two panics (`abs(PHP_INT_MIN)`, and the whole-i64 span). Now 31 of
34 match and the remaining three differ only in a float rendering that is also
fixed below.

- `$step` is validated first and independently of the bounds, with five distinct
  messages: `cannot be 0`, `must be greater than -9223372036854775808`, `must be
  a finite number, NAN|INF provided`, `must be greater than 0 for increasing
  ranges`, `must be less than the range spanned by …`.
- Each bound is classified by a port of `php_range_process_input`. A one-byte
  numeric string is AMBIGUOUS: read as a character when the other bound is also
  a string (`range("1","3")` → strings), as a number otherwise
  (`range("1.5","3")` → floats).
- An array bound is a `TypeError`, a null bound is a deprecation, an empty
  string warns `must not be empty, casted to 0`, and a whole-valued float step
  keeps an int range int.
- A span too large for a hash table is the reference's four-number
  `The supplied range exceeds the maximum array size by …` ValueError; the
  arithmetic is unsigned and wrapping, as the C's is.

**`array_fill()`** now distinguishes the C's four outcomes: negative `$count`,
`$count` past `INT_MAX`, zero `$count`, and a `$start_index` whose last key would
pass `PHP_INT_MAX` (checked before any element is written).

**Self-referential structures** — eight walkers exhausted the native stack.
Each now detects the repeat, the analogue of `GC_PROTECT_RECURSION`:

| walker | behaviour |
|---|---|
| `print_r` | prints the head, then ` *RECURSION*` in place of the block |
| `var_dump` | replaces the whole value, type header included |
| `var_export` | `Warning: var_export does not handle circular references`, writes `NULL` |
| `count($a, COUNT_RECURSIVE)` | `Warning: count(): Recursion detected`, counts the repeat once |
| `json_encode` | `false` with `json_last_error()` 6, `Recursion detected` |
| `http_build_query` | skips the repeat |
| `array_merge_recursive` / `deep_copy` | stops at the repeat |
| `serialize` | writes `N;` at the repeat (see BUGS.md — the reference emits a back-reference) |

**Allocation and slicing**

- `str_repeat` reproduces the reference's deterministic `Possible integer
  overflow in memory allocation (len * times + 32)` fatal — uncatchable, as it
  is there. New `fatals()` tag for engine-level failures that are not Throwables.
- `sprintf` width and precision are read under the C's `>= INT_MAX` rejection
  (`Width|Precision must be between 0 and 2147483647`) instead of accumulating
  into a `usize`.
- A float conversion caps `$precision` at 53 digits with the reference's
  `Notice: sprintf(): Requested precision of N digits was truncated to PHP
  maximum of 53 digits`, which is also what keeps `%.2147483646f` from building
  a two-gigabyte string.
- `round()` saturates `$precision` into the int range and keeps it off
  `INT_MIN`, and gained the C's `abs(places) >= 23` string round-trip so
  `round(1.5, PHP_INT_MIN)` is `0` rather than `NaN`.
- `strpbrk` cuts the byte vector instead of slicing the `&str`, so a match
  inside a multi-byte character no longer panics.
- `levenshtein` combines its three costs with wrapping arithmetic, matching the
  reference's own wrapped answer for an absurd cost.
- `mb_strcut` saturates `start + $length`; `mb_str_pad` no longer reserves
  `$length` up front.
- `json_decode`/`json_validate` range-check `$depth` (`> 0`, `<= INT_MAX`) and
  cap the native recursion at 1024.
- `usort`/`uasort`/`uksort`/`array_multisort` sort through a stable merge sort
  that never validates its comparator; Rust's `sort_by` panics on an
  inconsistent one, which PHP never does.
- `gmp_sqrt`, `gmp_root` and `gmp_perfect_square` test the sign BEFORE calling
  `BigInt::sqrt` (which asserts on a negative), and `gmp_root`/`gmp_pow`/
  `gmp_fact` gained the reference's range errors. `gmp_root` of a negative odd
  root now computes (`gmp_root("-8", 3)` is `-2`, was `0`).

**Uncatchable → catchable**

- Calling a non-callable is a catchable `Error` with the reference's three
  messages (`Array callback must have exactly two elements`, `Object of type C
  is not callable`, `Value of type T is not callable`).
- `Enum::from()` with no matching case is a catchable `ValueError`; an int
  needle renders bare, a string one quoted.
- `new` on an abstract class, an interface or an enum, and `new` on an unknown
  class, are catchable `Error`s naming the kind.

### Error shape

- **`JsonException` is implemented.** `JSON_THROW_ON_ERROR` is honoured by
  `json_encode`/`json_decode`/`json_validate`; the exception's `getCode()` is
  the `JSON_ERROR_*` constant and `json_last_error()` is left clean. New
  `throws_code()` tag for throws whose code is part of the contract.
- `sprintf` argument shortfalls report the class the reference reports:
  `ArgumentCountError` for loose parameters, `ValueError` for the `vsprintf`
  array form. The count is taken from the HIGHEST index a conversion wanted, so
  `sprintf("%")` reports a missing argument rather than a missing specifier.
- `sprintf` rejects an unknown conversion (`Unknown format specifier "z"`,
  including `%i`) and a missing one, swallows the `l` length modifier, and
  implements `%h`/`%H`.
- **String offset assignment is implemented.** `$s[1] = "Z"` edited nothing
  before — it replaced the whole variable with a one-element array. Ported from
  `zend_assign_to_string_offset`: in-range replace, space padding past the end,
  negative offsets from the end, `Illegal string offset -N`, `Only the first
  byte will be assigned…`, `Cannot assign an empty string to a string offset`,
  `Cannot access offset of type string on string`, `String offset cast
  occurred`.
- **`strpos()` honours `$offset`**, which it ignored entirely, and reports a
  BYTE offset. An offset outside `[-strlen, strlen]` is a `ValueError` in
  `strpos`, `stripos`, `strrpos` and `strripos`.
- New diagnostics at the reference's level, execution continuing: `chr()` and
  `ord()` deprecations, the `Invalid characters passed for attempted conversion`
  deprecation shared by `bindec`/`octdec`/`hexdec`, and `Array to string
  conversion` from `implode`.
- `bindec`/`octdec`/`hexdec` drop a base-matching `0b`/`0o`/`0x` prefix without
  a diagnostic, and `hexdec` now skips invalid characters instead of answering
  0 for the whole string.
- New `ValueError`s where the call previously returned a plausible value:
  `str_word_count` bad `$format`, `wordwrap` zero width with `$cut`,
  `mb_convert_encoding` unknown target. `array_rand` on an EMPTY array now names
  argument #1, not #2.
- Stack-trace arguments are escaped as `smart_str_append_escaped` escapes them
  (`\n`, `\t`, `\xHH` uppercase, `\\`, quote NOT escaped), truncated at 15
  BYTES, and a whole-valued float keeps its `.0` so it stays distinguishable
  from an int.
- `sys_get_temp_dir()` strips exactly one trailing separator, not the run.
- `INDEX_SET` is emitted with a line number, so a diagnostic from `$a[k] = v`
  reports line N instead of line 0.

### Tests

Every `#[test]` in `tests/` was censused for vacuous passes — a PASS with zero
assertions executed. 16 of 1229 were flagged and all 16 strengthened; nothing
was deleted.

- **`tests/ffi.rs` (the headline).** Both of the only two end-to-end FFI tests
  were gated by `if !rustc_available() { return; }`, and the probe honoured
  `$RUSTC`, so `RUSTC=/nonexistent cargo test` silently turned both into no-ops
  — one of them printing nothing at all. The probe now panics with a diagnostic
  instead of returning a bool, and a third test asserts the probe itself.
  Verified with a negative control: `RUSTC=/nonexistent` on the test binary
  fails all three rather than passing them.
- **`tests/corpus_coverage.rs`** — six gates derived a "bad" list and asserted
  it empty, with nothing checking the list was derived from anything. Each now
  carries a lower bound (per-table and whole-corpus name counts, chapter
  populations, `CORPUS.len()`), in the style `tests/opcodes.rs` already used.
- **Tautologies removed.** `superglobals.rs` asserted `count($_ENV) >= 0`, which
  is true of an empty `$_ENV`; it now seeds a variable and reads it back.
  `stdlib_math.rs` bounded `mt_rand()` by `mt_getrandmax()` read from the same
  engine; both are pinned and variation is asserted. `stdlib_fileio.rs`
  `getcwd()`/`disk_free_space()` and `magic_constants.rs` `__DIR__ === getcwd()`
  are anchored to the test process instead of to the engine's own answer.
  `cli_entry_points.rs` computed its expectation from the environment, so the
  `$argv[0]`-vs-`__FILE__` distinction went untested wherever the temp dir is
  not a symlink; it now forces the two apart with a `.` path segment.
- `tests/examples.rs` enumerates `examples/` and fails on a file with no test.
- One stale pin corrected: `stdlib_math.rs` expected `bindec('1a0b1')` to print
  `5` with no diagnostic, which was phplang's own behaviour and not the
  reference's.

Two new files, 33 new tests: `tests/degenerate_inputs.rs` (the aborts above) and
`tests/error_shape.rs` (class, `getCode()`, hierarchy, and diagnostic level
proved by masking the specific `E_*` bit).

### Incidental

`cargo clippy --all-targets -- -D warnings` was already failing on `main` before
this round. The three violations are fixed in code, with no `#[allow]` and no
lint-config change: `mem_replace_option_with_some` in `src/host.rs`, a
`repeat_n` call in `tests/opcodes.rs` newer than the declared MSRV, and an
unused `err` helper in `tests/visibility.rs` — now used by a new test that pins
uncaught visibility violations.
