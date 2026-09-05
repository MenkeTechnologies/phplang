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

## Round 10 — the syntax the fuzzer never generated

Measured under `PHP 8.5.10 (cli) (built: Aug 25 2026 21:09:32) (NTS)`; ini state
and environment as recorded in the oracle table above.

Round 7 and 8 found their bugs by listing the library functions no generator
mode emits. This round ran the same test over the SYNTAX. A grep for `<<<`,
`yield`, `enum `, `trait `, `...` and a `?->` past the first link returned ZERO
hits across 3,800 lines of generators — six constructs that every previous
"0 divergences" run scored not at all. Every one of them was carrying a bug, and
the first was not implemented at all.

### Heredoc and nowdoc are lexed

`<<<EOT` was `Parse error: syntax error, unexpected token "<<"`. The lexer now
has both forms:

* a heredoc body is the double-quoted language minus the `\"` escape — the
  reference leaves that backslash in place, because a `"` in a heredoc needs no
  escaping — so it shares `scan_interp` with `"…"` rather than a second copy of
  the interpolation and escape rules;
* a nowdoc body is verbatim, `\\` included;
* PHP 7.3's flexible closing delimiter is honoured: its indentation is stripped
  from every body line BEFORE anything is interpolated, an entirely empty line
  is legal at any level, and a line with less indentation is
  `Invalid body indentation level (expecting an indentation level of at least
  N)` naming that line;
* only the exact label closes a body (`EOTX` does not close `EOT`), the newline
  before the closing line belongs to the delimiter, and the label may be
  followed by any token — `<<<EOT\na\nEOT . "z";` is `"az"`.

### `?->` short-circuits the whole chain

```text
$ php -r '$n = null; var_dump($n?->a->b->c());'
NULL
```

phplang lowered each link with its own two-branch merge, so `->b` was read off
the null the first link produced: two `Attempt to read property` warnings and an
uncaught `Call to a member function c() on null`. A chain containing a `?->` is
now lowered as a unit, with one exit for every link that spells the operator, so
no later member, subscript or ARGUMENT is evaluated. A `?->` inside an argument
opens its own chain and keeps its own extent, so `$a->m($n?->x->y)` still calls
`$a->m`.

### `self` and `parent` inside a trait

A trait's methods are compiled once and copied into every class that uses them,
so the class they belong to is not known while the body is lowered — which is
why the parser already resolves `__CLASS__` there at run time. `self` and
`parent` did not: `self::class` answered the trait's name from every class,
`new self()` built an instance of the trait, and `parent::` refused every trait
that spelled it with `'parent' used in a class with no parent`. Both now resolve
from the running frame (`SELF_CLASS`, `PARENT_CLASS`), which is the composing
class exactly as PHP defines it.

### Interface constants are inherited

`interface I { const K = 5; } class C implements I {}` answered
`Error: Undefined constant C::K` from `C::K`, `self::K` and `static::K` alike —
the lookup walked the `parent` chain and nothing else. It now walks the parent
chain first (so a class constant still shadows an interface one at every depth)
and then the interfaces, transitively. `constant("C::K")` and `defined("C::K")`
did not read class constants at all, in any shape, and now go through the same
lookup.

### `...` unpacking at every call site

Unpacking was accepted in a call to a function named literally and refused
everywhere else with the compile-time `'...' argument unpacking is only valid in
a function call` — `$f(...$a)`, `$o->m(...$a)`, `C::s(...$a)` and
`new C(...$a)` were all hard failures. A spread now rides the same
`(name, value)` pair encoding named arguments use, with a marker in the name
slot that the host flattens, so all four sites take one and the string-keyed
form still binds by name.

### `Enum::from` / `Enum::tryFrom` coerce their argument

The needle was compared as its string rendering with no typing at all. The
reference coerces it to the backing type first, under the weak-mode rules for a
`string|int` parameter, and each step of that is observable:

```text
$ php -r 'enum E: int { case A = 1; } E::from("z");'
PHP Fatal error:  Uncaught TypeError: E::from(): Argument #1 ($value) must be of type int, string given
$ php -r 'enum E: string { case A = "a"; } try { E::from(9); } catch (ValueError $e) { echo $e->getMessage(); }'
"9" is not a valid backing value for enum E
```

A non-numeric string against an int-backed enum is a `TypeError` with a frame,
not a `ValueError`; `null` is the deprecation for passing null to a
`string|int` parameter and then `0`; a fractional float deprecates the
narrowing; and the failure message renders the value as the BACKING type, so a
string-backed enum quotes it. A pure enum has no `from` at all
(`Call to undefined method E::from()`).

### An object with `__toString` compared against a string

`zend_compare` casts it and compares the two strings. Without that `$obj == "s"`
was false, `$obj < "t"` fell to the object-vs-scalar rule, and `$obj <=> "s"`
was `1` for a `__toString` returning exactly `"s"`. The cast runs where all four
relational operators reach it — they are lowered natively, so it is applied in
the numeric hook as well as in the `==`/`<=>` builtins.

### `UnhandledMatchError` renders its subject as a trace does

The message concatenated the subject, so `null` became nothing at all, `true`
became `1`, `'hi'` lost its quotes, and an array became `Array` behind an
`Array to string conversion` warning the reference never raises. A scalar is now
rendered as a stack trace renders an argument and anything else is
`of type <name>`.

### `self` / `parent` / `static` in a closure

A closure is not compiled inside a class, so all three used to be the
compile-time refusal `php: 'self' used outside of a class` — which rejected a
program the reference RUNS, because `Closure::bind` and `->call()` give a
closure a class scope afterwards:

```text
$ php -r 'class C { const K = 1; } $f = function () { return self::K; }; echo $f->call(new C());'
1
```

They now resolve from the bound scope, and a closure called without one is the
reference's catchable `Error: Cannot access "self" when no class scope is
active` rather than a compile failure. A NAMED function keeps the compile-time
path — that is where the reference decides it too, refusing the program before
it runs any of it.

### Performance

Three changes on the string path, measured on this machine with the two
binaries built from the same tree and run alternately, each over a
uniquely-tagged source so the content-hash cache is cold for every measurement
(best of four):

| loop, 40k iterations | before | after |
|---|---|---|
| `$s .= "x"` | 0.19s | 0.14s |
| `$s = $s . "x"` | 0.30s | 0.13s |
| `$t = "x$i,"` | 0.20s | 0.13s |
| 50k `$a[] = $i` then `foreach` | 1.57s | 1.17s |
| `$s += $i` (no strings) | 0.02s | 0.02s |

* `PhpHost::to_str` CLONES a `Value::Str` instead of calling `to_string()` on
  it. They are not the same call: the payload is an `Arc<String>`, which
  `ToString` has no specialisation for, so every string reaching a
  concatenation, an interpolation, an array key or a library argument ran
  through `core::fmt`. A sampling profile of `$t = "x$i,"` spent most of its VM
  time in `Display::fmt`.
* `CONCAT` of two strings builds the result directly at the exact length,
  taking neither host borrow and no `format!`. Two strings have no
  `__toString` to run and no diagnostic to raise, which is what those borrows
  were for.
* `is_superglobal` gates on the first byte before its eleven comparisons. It is
  asked on every by-name variable access and was 3% of a `foreach` profile.

---

### Harness

* **The reported seed now replays.** A divergence recorded the case INDEX, while
  `--once --seed S` builds its case from `S` directly — so every "replays
  exactly" transcript this harness has ever printed rebuilt an unrelated program
  under an unrelated mode. The seed is what is recorded now.
* **`--mode NAME` generates that mode's cases** instead of generating every
  mode's and throwing away all but one in `MODES.len()`: a `--count 250` run of
  one mode used to compare one to four programs and report the rest as never
  having run. `--once` honours `--mode`, so a filtered finding replays.
* `PHPLANG_FUZZ_PHP` is refused unless it is a reference PHP, and `--once`
  prints the oracle it resolved. `tests/parity.rs` resolves an ABSOLUTE oracle
  (system paths before `PATH`, never one under `target/`) and prints its banner.
* Nine new modes: `heredoc`, `nullsafechain`, `matcherr`, `ifaceconst`,
  `generators`, `enums`, `traits`, `variadic`, `splobj`.

---

## Round 9 — the frames a library call occupies

Measured under `PHP 8.5.10 (cli) (built: Aug 25 2026 21:09:32) (NTS)`, a point
release past the 8.5.9 in the oracle table above; ini state and environment are
otherwise as recorded there.

`parity-fuzz --seed 60417 --count 25000` reported six divergences across two gap
classes, both of them the same mechanism: a trace captured inside an `array_map`
callback was two lines short of the reference's. Chasing that mechanism to its
edges turned up two further frame divergences that the fuzzer's generators do not
reach.

### A library function that runs a callback is a frame

PHP gives every internal function that invokes PHP code a frame of its own, and
the callback's frame reports `[internal function]` as its call site because there
is no PHP line to name:

```text
$ php -r 'array_map(function ($x) { throw new Exception("b"); }, [1]);'
#0 [internal function]: {closure:Command line code:1}(1)
#1 Command line code(1): array_map(Object(Closure), Array)
#2 {main}
```

phplang recorded neither line: the callback's frame carried the caller's own
site, and `array_map` was absent. Both halves now exist. A `Scope` can be marked
`internal`, `backtrace` renders the site of the frame above such a frame as
`[internal function]`, and `call_library_throwing` pushes one around the sixteen
library functions that can call back.

The set is a list rather than a rule because the reference is not uniform:
`call_user_func` and `call_user_func_array` invoke their callee from the CALLER's
frame, so neither appears in a trace and the callee reports the call site. Both
were measured and are deliberately excluded. Every other name was measured by
throwing out of its callback and reading the trace back — `array_map`,
`array_filter`, `array_reduce`, `array_walk`, `array_walk_recursive`,
`array_find`, `array_find_key`, `array_any`, `array_all`, `array_udiff`,
`array_uintersect`, `usort`, `uasort`, `uksort`, `preg_replace_callback` and
`iterator_apply`.

The rule is local: only the frame DIRECTLY above an internal one takes
`[internal function]`, so a user function called from the callback, and an inner
`array_map` called from it, both still report a real line. Pinned in
`tests/closure_frame_names.rs` against the reference, line for line.

### `Enum::from()` names its own call, in the declared spelling

```text
$ php -r 'enum MyLevel: int { case Low = 1; } MYLEVEL::FROM(99);'
Uncaught ValueError: 99 is not a valid backing value for enum MyLevel
#0 Command line code(1): MyLevel::from(99)
#1 {main}
```

phplang raised the `ValueError` from the caller's frame (`#0 {main}`) and echoed
the caller's casing back in the message (`enum MYLEVEL`). Class names are
case-insensitive and keyed lowercase, so a diagnostic that repeats the caller's
spelling reads wrong wherever the two differ; the reference always prints the
declaration's. `PhpHost::declared_class_name` recovers it, and the throw now goes
through `throw_from_internal`, which is what gives it the frame.

### A call the reference compiles to an OPCODE has no frame

The compiler turns a handful of calls into opcodes, so their argument errors are
raised where the call was written and no frame is pushed:

```text
$ php -r 'try { strlen([1]); } catch (Throwable $e) { echo $e->getTraceAsString(); }'
#0 {main}
$ php -r 'try { count([1], "x"); } catch (Throwable $e) { echo $e->getTraceAsString(); }'
#0 Command line code(1): count(Array, 'x')
#1 {main}
```

The specialisation is arity-exact AND name-exact, which is what pins the rule
rather than the guess: `count($x)` is an opcode but `count($x, $mode)` is a call,
and `key_exists` is never specialised even though `array_key_exists` always is.
Both edges were measured for every name. `strlen`, `count`, `sizeof` and
`get_class` at one argument and `array_key_exists` at two now raise frameless;
everything else is unchanged, including the seven names PHP specialises only for
a LITERAL argument (`chr`, `ord`, `defined`, `in_array`, `array_slice`,
`sprintf`, `intval`), all of which were measured framed.

The specialisation also depends on HOW the call was reached, which the first
cut of this missed. The compiler can only specialise a call it can see, so the
same name dispatched through a value is an ordinary internal call:

```text
$ php -r 'try { strlen([1]); }                   catch (Throwable $e) { echo $e->getTraceAsString(); }'
#0 {main}
$ php -r 'try { call_user_func("strlen", [1]); } catch (Throwable $e) { echo $e->getTraceAsString(); }'
#0 Command line code(1): strlen(Array)
#1 {main}
$ php -r 'try { array_map("strlen", [[1]]); }    catch (Throwable $e) { echo $e->getTraceAsString(); }'
#0 [internal function]: strlen(Array)
#1 Command line code(1): array_map('strlen', Array)
#2 {main}
```

`call_function` therefore carries a `Dispatch`, which `call_value` — the single
entry point for `$f(…)`, every callback, and `call_user_func` — sets to
`Indirect`. All three transcripts above now match.

### Five `json_encode` flags were accepted and discarded

BUGS.md carried a standing task: `JSON_PRETTY_PRINT` was documented twice, once
as honoured and once as "not honoured — the encoder always emits the compact
form", and neither statement had been checked. Checking it turned up eleven
constants whose corpus text was wrong in one direction or the other, and five
flags that really did nothing.

`JSON_HEX_TAG`, `JSON_HEX_AMP`, `JSON_HEX_APOS` and `JSON_HEX_QUOT` now escape
their character. All four spell their hex digits in UPPER case where the control
and unicode escapes in the same string spell theirs in lower, which is why they
cannot share the general escape path — in the transcript below `<` becomes
`\u003C` while the `é` beside it becomes `\u00e9`:

```text
$ php -r 'echo json_encode(["<x", "é"], JSON_HEX_TAG);'
["\u003Cx","\u00e9"]
```

`JSON_NUMERIC_CHECK` now encodes a numeric string as its number. The test is
PHP's own `is_numeric_string`, so leading and trailing whitespace are allowed
(`" 5"`, `"5 "`) while `"0x1A"`, `"0b1"` and `"1_0"` are not numeric. The number
goes through the same rendering a real float takes, which drops an integral
value's fractional part — `["1e3"]` encodes as `[1000]`, the way `[1000.0]`
already did — and a string that reads as a non-finite double (`"1e999"`) has no
JSON spelling and stays a string. Array KEYS are untouched; JSON has no
non-string key.

### Eleven constants were documented as not working when they work

Measured one at a time against the reference, and corrected in `src/corpus.rs`,
which is what `docs/reference.html` and the reference manual are generated from:
`PHP_ROUND_HALF_EVEN`, `PHP_ROUND_HALF_ODD` and `round()`'s own `$mode` note;
`SORT_FLAG_CASE`; `JSON_UNESCAPED_SLASHES`, `JSON_PRETTY_PRINT` and
`JSON_UNESCAPED_UNICODE`; and the five flags above, whose entries were right
before this commit and are wrong after it. `JSON_PRETTY_PRINT`'s worked EXAMPLE
claimed a compact `{"a":1}` for a call that indents.

The two remaining "not honoured" claims were re-measured and left standing:
`JSON_BIGINT_AS_STRING` is genuinely not read, and `FILE_USE_INCLUDE_PATH` has
no include path to search.

### `sizeof()` blamed a function the program never called

`count` and `sizeof` share one implementation, which hardcoded `count()` into
both of its own error messages, so every `sizeof()` failure named the wrong
function. Both now blame the name the caller wrote — the reference's rule for
every alias, and one `argtypes` was already following for `key_exists` and
`chop`.

---

## Round 8 — the scanner, the charmask, and the array-shaped arguments

Chosen by the same method that found round 7's gaps: cross-referencing the 62
`parity_fuzz` generator modes against the registered library surface. 341 of the
511 registered functions had no generator hit at all, and the families picked out
of that list — `sscanf`, the `php_charmask` consumers, `count_chars`, `strtok`,
the array forms of `substr_replace`, `substr_compare`'s case flag, the recursive
array pair, and the `array_sum`/`array_product` fold — held eleven measured
divergences between them. Five new generator modes now cover them
(`sscanf`, `cslashes`, `strtokcounts`, `substrx`, `arrayfold`), and those modes
found three further divergences within minutes of being written.

### `sscanf` rewritten as a port of `php_sscanf_internal`

The previous implementation handled `%d %i %f %e %g %s %c %%` and dropped
everything else on the floor, silently returning a SHORT or EMPTY array.

- `%x`, `%o`, `%u` and `%i`'s base auto-detection now exist. Each was previously
  an unrecognized specifier that aborted the whole scan, so
  `sscanf("ff 10 0x1F", "%x %o %i")` answered `[]` instead of `[255, 8, 31]`.
- `%[…]` scan sets now exist, including `^` negation, `a-z` ranges, and the two
  placement quirks `BuildCharSet` has (a leading `]` is a member, a trailing `-`
  is a literal).
- `%n` (byte offset, consumes nothing) and the `l`/`L`/`h` size modifiers
  (parsed and ignored) now exist, as does `*` assignment suppression.
- The result array is now PRE-FILLED with one null per non-suppressed specifier,
  so a format that outruns its input pads rather than truncating:
  `sscanf("a b", "%s %s %s")` is `["a", "b", null]`, not `["a", "b"]`.
- Underflow with zero conversions is now distinguished from a mismatch, and
  answers `null` (two-argument form) or `-1` (by-reference form).
- **The by-reference form now exists.** `sscanf($s, $fmt, $a, $b)` previously
  warned `Undefined variable $a`, returned the array, and wrote nothing back.
  It now returns the conversion count and assigns the variables — and a variable
  no conversion reached is left UNTOUCHED rather than nulled.
- Its arity is validated against the format before any input is read, raising
  `ValueError: Variable is not assigned by any conversion specifiers` or
  `ValueError: Different numbers of variable names and field specifiers`.

The compiler grew a variadic by-reference builtin table for this. The existing
table maps a name to fixed positions; `sscanf` takes every argument from index 2
on, so the positions are a property of the call. The write-back is emitted
GUARDED (`ops::BYREF_LIVE`), which is what preserves the untouched-variable rule.

### `php_charmask` unified, with its four diagnostics

Three separate range parsers existed (`trim_char_set` in `builtins.rs`,
`charmask` in `stdlib::misc`, and none at all for the new `addcslashes`), and
none of them raised any of the four malformed-range warnings. They are now one
port of `php_charmask` in `stdlib::common`, threaded through the host so the
message names whichever function is running:

```text
$ php -r 'echo trim("a..b", "z..a");'
Warning: trim(): Invalid '..'-range, '..'-range needs to be incrementing …
```

`trim`/`ltrim`/`rtrim` also became byte-oriented, as `php_trim_int` is.
`str_word_count` now answers an empty subject BEFORE building the mask, matching
upstream's early return — so a malformed range there is silent when there is
nothing to scan.

### Five string functions that did not exist

`addcslashes`, `stripcslashes`, `count_chars`, `strtok` and
`array_replace_recursive` were all `Call to undefined function`. Each is a port;
`strtok` carries its tokenizer state on the host, including the rule that
running out of tokens DISCARDS the subject so later one-argument calls keep
answering `false` instead of restarting.

### Array-shaped arguments and unnormalized comparisons

- `substr_replace` accepted only the all-scalar form. An array subject was
  stringified to `"Array"` and spliced, so `substr_replace(["ab","cd"], "Z", 1, 1)`
  answered the STRING `"AZray"` instead of `["aZ", "cZ"]`. All four parameters
  may now be arrays, consumed positionally; an array `$offset` or `$length`
  against a single string is now the `TypeError` upstream raises.
- `substr_compare` ignored `$case_insensitive` entirely, and normalized its
  result to -1/0/1. It now honours the flag and returns the raw byte difference
  (`substr_compare("abc","abz",0,3)` is `-23`), falling back to the three-way
  length comparison only on a content tie, with the two `ValueError`s for a bad
  offset or a negative length.
- `array_walk_recursive` passed leaves by value, so a `function (&$v)` callback
  could not write back. It now uses the same reference-cell plumbing `array_walk`
  already had.
- `array_sum`/`array_product` coerced every entry silently. Following
  `php_array_binop`, an operand `+`/`*` rejects now warns
  `<op> is not supported on type <type>`; an array or an object with no numeric
  cast contributes nothing, while a non-numeric string keeps the pre-8 behaviour
  of counting as `0` — which is why `array_product([2, "a"])` is `0`, not `2`.
- `str_getcsv` now raises PHP 8.4's deprecation when `$escape` is omitted. Six
  existing tests encoded the pre-deprecation output; they were re-pointed at the
  reference's measured output and extended with an explicit-`$escape` form that
  proves the notice is the only difference.

### Not closed

`quoted_printable_decode`, `hex2bin`, `base64_decode` and `count_chars` modes 3
and 4 all diverge for the same architectural reason — `Value::Str` is a Rust
`String`, so a byte above 0x7F widens to two. Recorded in
[BUGS.md](BUGS.md#a-non-ascii-byte-cannot-be-represented) rather than papered
over. The `{closure}` vs `{closure:file:line}` stack-frame naming gap is
unchanged and is still the only divergence a 25,000-case full-corpus fuzz run
reports.

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
