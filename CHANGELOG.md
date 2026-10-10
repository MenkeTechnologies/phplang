# Changelog

Behavioural changes to the engine, newest first. Everything here is measured
against the reference implementation; see [BUGS.md](BUGS.md) for divergences
that are known and **not** fixed.

## Reference oracle

Every expectation in this file and in `tests/` comes from a recorded run of:

| | |
|---|---|
| binary | `/opt/homebrew/bin/php` → `/opt/homebrew/Cellar/php/8.5.10/bin/php` |
| version | `PHP 8.5.10 (cli) (built: Aug 25 2026 21:09:32) (NTS)`, Zend Engine v4.5.10 |
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

## Round 23 — differential sweep: operand classes, typed constants, mbstring

Measured under `PHP 8.5.11 (cli)`. Found by a fresh `parity-fuzz` baseline, a
language-feature sweep over PHP 8.2-8.5 additions, and a cross-reference of
`get_defined_functions()` against `function_exists()`. Each fix has a block in
`tests/data/parity_corpus.php`.

* **Operand order of `*`, `|`, `&`, `^` follows operand classes.** The
  reference specialises a commutative handler with the lower-ranked operand
  (constant < temporary < call result < plain variable) in the second slot, so
  `[1] * $x`, `f() * $x` and `[$y] * g()` report `int * array`, while `$a * f()`
  and `f() * ($x + 0)` keep source order. Only a scalar literal on the left was
  modelled before. Calls the reference folds or compiles to a dedicated opcode
  are classified separately.
* **Typed class constants (8.3).** `const int X = 1;`, `final public const
  ?string S = null;` and typed constants in interfaces and enums parse; a
  literal initialiser the type refuses is the compile-time `Cannot use string as
  value for class constant A::X of type int`, `void`/`never`/`callable` are
  refused, and an `int` literal under a `float` type holds `1.0`.
* **Typed property defaults are checked at compile time.** `public int $x =
  "a";` is `Cannot use string as default value for property A::$x of type int`,
  and `public int $x = null;` is the reference's `may not be null` fatal with
  the nullable spelling. `public float $x = 1;` holds a float.
* **Dynamic class constant fetch (8.3)**, `A::{$name}`, `$obj::{$name}` and
  `static::{$name}`. `constant("self::X")`, `"static::X"` and `"parent::X"`
  resolve against the calling scope.
* **`mb_trim`, `mb_ltrim`, `mb_rtrim` (8.4), `mb_strstr`, `mb_stristr`,
  `mb_strrchr`, `mb_strrichr`, `mb_strimwidth`, `mb_encode_numericentity` and
  `mb_decode_numericentity`** were missing. `mb_strimwidth` deprecates a
  negative width (8.5).
* **`number_format(null)`** deprecates the parameter as `float`, not
  `int|float`. The pinned expectation in `tests/builtin_arity.rs` said
  `int|float`; the reference prints `float`.

---

## Round 22 — differential sweep: parse errors, traces, GMP, hashing, fnmatch

Measured under `PHP 8.5.11 (cli) (built: Sep 22 2026 13:32:06) (NTS)`. Found by
the library-surface cross-reference in the README's parity-fuzzer chapter and by
five new `parity-fuzz` modes (`fnmatchflags`, `gmpops`, `hashalgos`, `ctypeargs`,
`varexport`) plus `parseerr`, which inserts a stray operand token into valid
programs. Each fix has a block in `tests/data/parity_corpus.php`.

* **`getTrace()` did not exist and `debug_backtrace()` returned `[]`.** The
  rendered trace string was the only thing kept. Frames are now built in the
  structured form (`file`, `line`, `function`, `class`, `type`, `args`, with
  `file`/`line` absent for a frame entered from internal code, and `args`
  before `function` for an `include`/`eval` frame); `getTraceAsString()`
  renders that array, `debug_backtrace()` reports it with the `object` key
  under `DEBUG_BACKTRACE_PROVIDE_OBJECT` and honours
  `DEBUG_BACKTRACE_IGNORE_ARGS` and the limit, and `debug_print_backtrace()`
  prints it. `Exception`/`Error` keep `string`, `trace` and `previous` as the
  reference's private properties in its declaration order.
* **A parse error now carries `, expecting …` where the reference prints one**:
  after an argument, array element, parameter, `use` item, `echo`/`return`/
  `break`/`continue` operand, `global`/`static`/`const`/property declaration,
  `for` header, `catch` header, a missing `(`, `{`, `=>`, `=`, `while`, a class
  member that opens with no modifier (`expecting "function"`), and the
  `unset`/`foreach` operand chain; `empty($a $b)` and `if ($a $b)` correctly
  print none. The scanner's bracket diagnostics (`Unmatched ')'`, `Unclosed '['
  does not match ')'`) are reproduced exactly. `break $x` is the compile-time
  `'break' operator with non-integer operand is no longer supported`,
  `class A { int $a; }` and `class A { case X; }` are refused, and `namespace;`
  and `function f` without a name or `(` no longer parse.
* **`$undefined;` raised a warning.** A bare variable statement compiles to no
  opcode in the reference; the read is no longer performed.
* **The next append key after a negative key is that key plus one** (8.3):
  `[-3 => 'a', 'b']` holds `-3` and `-2`; an untouched array still starts at 0.
* **`fnmatch` is the libc matcher**: `FNM_PERIOD`, `FNM_PATHNAME` and
  `FNM_NOESCAPE` were ignored, backslash escapes and `[[:class:]]` did not
  exist, and an unterminated bracket or trailing backslash matched.
  `glob()` shares it.
* **GMP**: `gmp_div_q`/`gmp_div_r`/`gmp_div_qr` take `GMP_ROUND_*` (`gmp_div_r`
  was `gmp_mod`), `gmp_init` and `gmp_strval` take every base in range, an
  operand that is not an integer string is the reference's `ValueError` for that
  parameter, `gmp_intval` wraps like `mpz_get_si`, and `gmp_gcdext`,
  `gmp_invert`, `gmp_sqrtrem`, `gmp_rootrem`, `gmp_binomial`, `gmp_nextprime`,
  `gmp_perfect_power`, `gmp_jacobi`/`legendre`/`kronecker`, `gmp_popcount`,
  `gmp_hamdist`, `gmp_testbit`, `gmp_scan0`/`scan1`, `gmp_com` and
  `gmp_divexact` were added.
* **Hashing**: one algorithm table now serves `hash`, `hash_hmac`, `hash_file`,
  `hash_pbkdf2` and `hash_algos`; `md4`, `sha224`, `sha512/224`, `sha512/256`,
  `sha3-*`, `adler32`, `crc32c`, `fnv*` and `joaat` were added, and `hash("sha384")`
  no longer throws.
* **Deprecations**: `ctype_*` on a non-string raises `Argument of type T will be
  interpreted as string in the future`; `utf8_encode`/`utf8_decode` raise their
  8.2 function deprecation.
* **Smaller**: `dirname("")` is `""` and `pathinfo("")` has no `dirname`;
  `var_export` spells `PHP_INT_MIN` as `-9223372036854775807-1` and splices NUL
  bytes in as `'' . "\0" . ''`; a diagnostic raised inside an arrow function
  names the line of its `fn`.

## Round 21 — sort algorithm, user-comparator array family

Measured under `PHP 8.5.11 (cli) (built: Sep 22 2026 13:32:06) (NTS)`; ini state
and environment otherwise as recorded in the oracle table above. Each fix has a
block in `tests/data/parity_corpus.php`.

* **Every sort ran a different algorithm from the reference.** `sort`,
  `rsort`, `asort`, `arsort`, `ksort`, `krsort` used Rust's `sort_by`, which
  PANICS on a comparator that is not a total order — any mixed-type
  `SORT_REGULAR` sort such as `sort(["10", 9, "9a", "abc", true, null])` killed
  the process — and `usort`/`uasort`/`uksort`/`array_multisort` used a merge
  sort, so an echoing comparator saw different pairs in a different order. All
  of them now run `zend_sort` (`Zend/zend_sort.c`, ported in
  `src/stdlib/zsort.rs`) with the original-position tie break of
  `RETURN_STABLE_SORT`; the reverse sorts negate the forward answer instead of
  swapping operands, as `php_array_reverse_*` does. A comparator returning a
  bool raises `Returning bool from comparison function is deprecated` once per
  call and a `false` is retried with the operands swapped
  (`php_array_user_compare_unstable`).
* **The user-comparator diff/intersect family was half missing and walked
  differently.** `array_udiff_assoc`, `array_uintersect_assoc`,
  `array_diff_uassoc`, `array_intersect_uassoc`, `array_udiff_uassoc` and
  `array_uintersect_uassoc` did not exist, and `array_udiff`,
  `array_uintersect`, `array_diff_ukey` and `array_intersect_ukey` scanned each
  operand linearly per probe — a different set of comparator calls in a
  different order — and returned `[]` for fewer than three arguments. All ten
  are now ports of `php_array_diff` / `php_array_intersect` (`zend_sort` every
  operand, walk the sorted lists in step, with the C's swapping of the active
  callback) or, for the two `_assoc` forms, `php_array_diff_key` /
  `php_array_intersect_key`; the `"+f"`/`"+ff"` parameter errors
  (`expects at least N arguments`, `Argument #N must be a valid callback`,
  `Argument #N must be of type array`) are raised in the reference's order.
* **A temporary was writable.** `(new A)->x = 1`, `[1, 2][0] = 3`,
  `A::C[0] = 1`, `unset((new A)->x)`, `(new A)->x++` and `$r = &(new A)->x` ran
  (or died with an internal `unsupported assignment target`); the reference
  refuses each at compile time with `Cannot use temporary expression in write
  context` (`zend_compile_var_inner`), because the container of a written
  element or property is compiled for write down the whole chain. The same
  check reports `(clone $o)->x = 1` (`Cannot use result of built-in function in
  write context`), `f() = 1` / `$o->m() = 1` (`Can't use function|method
  return value in write context`), a `?->` anywhere in a written chain (`Can't
  use nullsafe operator in write context`) and `$r = &$a?->b` (`Cannot take
  reference of a nullsafe chain`). `tests/classes.rs` wrote a readonly property
  through `(new P(1))->a = 2`, which the reference rejects before running; it
  now writes through a variable, with the reference's output unchanged.
* **Output buffering ignored its handlers.** `ob_start($callback, $chunk_size,
  $flags)` read none of its arguments, so no handler ever ran; failures were
  silent; `ob_clean`, `ob_get_status`, `ob_list_handlers`, `ob_implicit_flush`
  and the `PHP_OUTPUT_HANDLER_*` constants were missing; and buffers left open
  were flushed before the shutdown functions and destructors rather than after.
  The stack is now a port of `main/output.c` (`src/stdlib/output.rs`): handlers
  get the `$phase` bits, `false` disables a handler and passes its buffer
  through, `true`/`""` swallow it, `ob_get_clean`/`ob_end_clean` still run it,
  a filled chunk runs it, the ability flags gate each operation with the
  reference's notices, output written inside a handler is the 8.4
  deprecation, an output operation inside one is the fatal
  `Cannot use output buffering in output buffering display handlers`, and the
  end of the request finalizes every buffer after the shutdown functions and
  destructors. A closure handler is named `{closure:<file>:<line>}`, as
  `zend_get_callable_name_ex` names it.
* **The engine reported itself as PHP 8.3.0.** `PHP_VERSION`, the
  `PHP_*_VERSION` parts, `PHP_VERSION_ID` and `phpversion()` now all derive from
  one constant, the reference's version, so `version_compare(PHP_VERSION,
  "8.4", ">=")` takes the reference's branch. `defined()`/`constant()` answer
  `true`/`false`/`null` in any case and strip a leading `\`
  (`zend_get_constant_str_impl`), and the Core/standard build constants
  `PHP_EXTRA_VERSION`, `PHP_DEBUG`, `PHP_ZTS`, `ZEND_THREAD_SAFE`,
  `ZEND_DEBUG_BUILD`, `PHP_MAXPATHLEN`, `PHP_FD_SETSIZE`, `PHP_SHLIB_SUFFIX`,
  `M_LNPI`, `INI_*`, `CONNECTION_*`, `PHP_QUERY_*` and `UPLOAD_ERR_*` exist.
  `tests/superglobals.rs` asserted `8.3.0`; it now asserts the reference's
  major.minor from `tests/data/parity_reference_version.txt`.
* **An `int` parameter was not `zend_parse_arg_long_weak`.** A float with a
  fraction, or a float-form numeric string, was accepted without the
  `Implicit conversion from float … to int loses precision` / `from
  float-string "…"` deprecation; NaN, the infinities and anything outside
  `[-2^63, 2^63)` were wrapped instead of refused with `must be of type int,
  float|string given` (`str_repeat("a", NAN)` aborted the process on the
  allocation); and a numeric string with whitespace or a fraction read as 0 in
  the functions that narrowed with `Value::to_int` (`str_repeat("ab", "2.5")`
  was `""`). The declared-type check now ports the weak long parse for every
  `int` parameter it describes, and those functions narrow through
  `host::long_of`.
* **`ip2long`, `long2ip`, `fpow` and `strcoll` were missing.** `ip2long` parses
  with the platform's `inet_pton` and `strcoll` returns the C library's raw
  answer under the current `LC_COLLATE`, as the reference does — so a leading
  zero in an address (`"01.2.3.4"`) is accepted on macOS and refused on glibc,
  and `strcoll`'s magnitude differs between the two.
* **`fgetcsv` and `fputcsv` were missing, and `str_getcsv` was an
  approximation.** All three are now `php_fgetcsv` / `php_fputcsv` from
  `ext/standard/file.c` (`fgetcsv` reads further lines while an enclosure is
  open), with the reference's argument checks in its order: a separator or
  enclosure that is not one byte is a `ValueError`, as is an escape longer than
  one, and the omitted-escape deprecation comes after them.

## Round 20 — prelude closures and frames, SPL iterators

Measured under `PHP 8.5.11 (cli) (built: Sep 22 2026 13:32:06) (NTS)`; ini state
and environment otherwise as recorded in the oracle table above. Each fix has a
block in `tests/data/parity_corpus.php`.

* **A closure written in the prelude was replaced by the program's first
  closure.** Both were compiled under the name `@closure1`, and the prelude is
  merged before the program, so an `SplHeap` whose `compare()` threw failed with
  an `ArgumentCountError` from the user's closure once the program declared
  one. Prelude closures now have names of their own.
* **A trace showed the prelude's helper frames.** A prelude method stands in
  for an internal one: the frames it opens for itself (private helpers,
  closures, library calls) are left out, and a user method it calls back into
  (`compare()`, `accept()`) is entered from `[internal function]`, as
  `zend_fetch_debug_backtrace` reports a call made by internal code.
* **`foreach` over an `Iterator` object ran the whole iteration before the
  body.** The subject was materialized into an array first, so a body never saw
  the iterator mid-walk (`CachingIterator::hasNext()` was always false) and an
  infinite iterator hung. A by-value `foreach` now drives `rewind`, `valid`,
  `current`, `key` and `next` interleaved with the body, as
  `zend_fe_fetch_object_helper` does, binding the value before the key; an
  `IteratorAggregate` hands over the iterator its `getIterator()` answers
  (through nested aggregates), and one answering a non-`Traversable` is the
  reference's `Exception`. Each step's frame names the `foreach` line.
* **The SPL iterators were missing.** `IteratorIterator`, `FilterIterator`,
  `CallbackFilterIterator`, `RecursiveFilterIterator`,
  `RecursiveCallbackFilterIterator`, `ParentIterator`, `LimitIterator`,
  `CachingIterator`, `RecursiveCachingIterator`, `NoRewindIterator`,
  `InfiniteIterator`, `AppendIterator`, `EmptyIterator`, `RegexIterator`,
  `RecursiveRegexIterator`, `RecursiveIteratorIterator`, `RecursiveTreeIterator`
  and `RecursiveArrayIterator` are ports of `ext/spl/spl_iterators.c` (and
  `spl_array.c`'s recursive array iterator): the dual iterator's fetch/free
  cycle, the limit seek, the caching look-ahead and its flags, the regex modes,
  `spl_recursive_it_move_forward_ex`'s state machine with its hooks and
  `CATCH_GET_CHILD`, and the tree prefixes, with the reference's messages. An
  iterator the reference walks natively (`ArrayIterator`, the recursive iterator
  iterators, the SPL lists and heaps) opens no frame in a trace when a
  `foreach` steps it, and `iterator_to_array` / `iterator_count` are frames of
  their own. `ArrayIterator::getFlags()` answers the flags it was given.
* **`#[\Deprecated]` was parsed and ignored.** A function or method carrying
  it now raises `zend_deprecated_function`'s `Function f() is deprecated` /
  `Method K::m() is deprecated`, with ` since <since>` and `, <message>` from
  the attribute's literal arguments, at the call's line and as
  `E_USER_DEPRECATED` — before the callee binds its arguments, with an error
  handler run on the spot, so a handler that throws stops the call.
* **`SplObjectStorage` was a stub** with no array access, iteration, `getInfo`,
  `addAll`/`removeAll`/`removeAllExcept`, `seek`, `getHash` or serialization.
  It is now a port of `ext/spl/spl_observer.c`: objects keyed by handle or by a
  subclass's `getHash()`, in insertion order, the internal pointer and index
  `seek` walks, `Object not found`, and the reference's `__debugInfo` and
  `__serialize` views. Its 8.5 deprecations of `attach`/`detach`/`contains`
  are withheld (see BUGS.md).
* **`$x instanceof $c` was a parse error.** The class may now be given by
  an expression — a variable, an element, a property, a static property, or a
  parenthesised expression — which stands for its class when it is an object
  and names one when it is a string; anything else is `Error: Class name must
  be a valid object or a string`.
* **A generator could be rewound and re-traversed after running.**
  `rewind()` past the first `yield` is `Exception: Cannot rewind a generator
  that was already run`, and a `foreach` or `iterator_to_array` over a finished
  generator is `Cannot traverse an already closed generator`
  (`zend_generator_rewind`, `zend_generator_get_iterator`).
* **`get_class_methods` listed every method in hash order.** It now follows
  the function table: the class's own methods, then its traits', then each
  ancestor's, with private and protected ones only where the calling scope may
  call them, the prelude's helpers left out, an enum's `cases`/`from`/`tryFrom`,
  and the reference's `TypeError` for an undeclared class. Names stay
  lowercased (see BUGS.md).

## Round 19 — `Throwable::__toString`, the SPL data structures

Measured under `PHP 8.5.11 (cli) (built: Sep 22 2026 13:32:06) (NTS)`; ini state
and environment otherwise as recorded in the oracle table above. Each fix has a
block in `tests/data/parity_corpus.php`.

* **`Exception::__toString` / `Error::__toString` returned only the message.**
  They are now a port of `ZEND_METHOD(Exception, __toString)`
  (`Zend/zend_exceptions.c`): `Class: message in file:line`, `Stack trace:` and
  the object's own `getTraceAsString()` (`#0 {main}` when that is empty), for
  every throwable down the `previous` chain, innermost first and joined by
  `Next `. An empty message drops the `: `; a user `TypeError` /
  `ArgumentCountError` whose message has `, called in ` gains ` and defined`.
* **An uncaught exception ignored its `previous` chain and any user
  `__toString`.** The fatal is now `Uncaught <__toString()>` followed by
  `thrown in`, as `zend_exception_error` builds it, and an exception thrown by
  that `__toString` is reported in its place, from an `[internal function]`
  frame.

* **`__debugInfo` was ignored.** `var_dump` and `print_r` now show an object
  through `zend_std_get_debug_info`: the array `__debugInfo()` returns, its
  `"\0Class\0p"` / `"\0*\0p"` keys labelled private / protected and an integer
  key printed bare; `null` is the reference's deprecation and an empty dump.
* **`serialize` ignored `__serialize` and `unserialize` ignored
  `__unserialize`.** `serialize` writes the array `__serialize()` answers,
  keys as written, and a non-array answer is the reference's `TypeError`;
  `unserialize` allocates such an object with its defaults and no
  constructor, and hands `__unserialize()` its data once the whole payload has
  parsed, in creation order.
* **An exception or diagnostic raised inside a PHP-written prelude method
  named the prelude's own source line** (`ArrayIterator::seek` past the end
  reported line 340). Both now name the first user frame's line, as an
  internal function's do.
* **`SplDoublyLinkedList`, `SplQueue`, `SplStack`, `SplHeap`, `SplMinHeap`,
  `SplMaxHeap`, `SplPriorityQueue` and `SplFixedArray` are ports of
  `ext/spl`.** They were loose stand-ins: `SplStack` iterated FIFO, the heaps
  linear-scanned and broke ties in insertion order, `setIteratorMode`,
  `add`, `prev`, `setExtractFlags`, `recoverFromCorruption` and the
  serialization hooks were missing, `SplHeap` was concrete, and no bounds or
  emptiness check threw. They now carry the reference's iterator modes,
  sift-up / delete-top walks (so equal priorities come out in the
  reference's order), corruption after a throwing `compare()`, exception
  messages, `__serialize` shapes and debug views. `tests/foreach_object.rs`
  expected `SplStack` to iterate `abc`; the reference prints `cba`, and the
  expectation now records that.

## Round 18 — operator precedence, the default random engine, `new $class`

Measured under `PHP 8.5.11 (cli) (built: Sep 22 2026 13:32:06) (NTS)`; ini state
and environment otherwise as recorded in the oracle table above. Every
expectation added this round was recorded from that binary.

* **`.` bound as tightly as `+`.** PHP 8 moved concatenation below `+`/`-` and
  `<<`/`>>`; phplang still parsed `"a" . 1 + 2` as `("a" . 1) + 2`, a
  `TypeError`, where the reference answers `"a3"`.
* **`number_format` with a negative `$decimals`** formatted as if it were 0. It
  now rounds to the power of ten, and an `int` argument takes the reference's
  integer path, so `number_format(PHP_INT_MAX)` keeps its last digits and
  `number_format(PHP_INT_MAX, -1)` is `9,223,372,036,854,775,810`.
* **`iterator_to_array($it, false)` and `iterator_count` lost elements** whose
  keys repeated (`yield from` restarts its keys at 0): both read the key-merged
  array a `foreach` builds instead of the element sequence.
* **The randomizing functions are the reference's Mt19937.** `rand`, `mt_rand`,
  `shuffle` and `array_rand` drew from two unrelated generators (SplitMix64 and
  xorshift64), so `mt_srand($seed)` reproduced nothing. They now share one port of
  `ext/random/engine_mt19937.c` with the reference's range reduction
  (`php_random_range32`/`64`), shuffle walk and `array_rand` bitset pick.
  `str_shuffle` was missing and is added on the same engine.
* **The `${var}` deprecation named the wrong line** in a multi-line string: the
  reference attributes it to the start of the text run before the `${`.
* **`ErrorException`'s constructor** now follows the reference's file/line rule:
  a `$filename` replaces the line too (`0` when `$line` is null).
  `AssertionError` is declared.
* **`new $class` was a parse error**, with every other run-time class operand:
  `new ($expr)`, `new $o->prop`, `new $a['k']`, `new C::$prop`, `new $$n`. The
  operand is the grammar's `new_variable`, so the `(` that follows is the
  constructor's argument list.
* **`foreach` over an object visited every property**, private ones included,
  from any scope. It now visits what the calling scope may see.
* **A NaN bound to a `string` or `bool` parameter** in weak mode now warns as
  the reference does.
* **`\p{Lu}`, `\x{41}`, `\g{1}` and `\k{n}` did not compile**: the escape's
  braces reached the `{n,m}` rewrite. `preg_replace_callback_array` was missing.

## Round 17 — generator destruction, property writes on non-objects, by-reference method arguments

Measured under `PHP 8.5.11 (cli) (built: Sep 22 2026 13:32:06) (NTS)`; ini state
and environment otherwise as recorded in the oracle table above. Every
expectation below was byte-diffed against that binary (stdout, stderr and exit
status). Each fix has a block in `tests/data/parity_corpus.php`.

**A generator destroyed while suspended never ran its `finally`.** Leaving a
`foreach` over a generator its subject expression created — by `break`,
`break N` or `return` — now destroys it on the spot, unwinding the body from
the parked `yield` as a `return;` would: `finally` blocks run, `catch` blocks
do not, and a `yield` reached in such a `finally` is the reference's `Cannot
yield from finally in a force-closed generator`. Without reference counts,
"nothing else holds it" is answered by scanning the heap and every frame for
the handle, and only for a generator created while the subject was evaluated.
At request end every generator still suspended is destroyed — after `exit`
and after an uncaught exception's fatal too, with the `exit` status kept.

**A property write on something that cannot hold one was a silent no-op.**
`$x->p = v`, `$x->p .= v`, `$x->p++` and `$x->p[] = v` on null, a bool, a
number, a string, an array or a resource now throw the reference's `Attempt to
assign|increment/decrement|modify property "p" on <type>` (a bool named by its
value), and on a closure or generator `Cannot create dynamic property
Closure::$p`. A plain `=` fetches its container for WRITING, as the reference
does: an undefined receiver variable raises no `Undefined variable`, and a
missing link in `$o->a->b = v` is created as null without an `Undefined
property` warning, so the write after it is what fails. A `__get` link that is
not an object raises `Indirect modification of overloaded property`.

**A by-reference method parameter accepted a literal.** `$o->m(1)` and
`C::m(1)` on `function m(&$a)` are now `C::m(): Argument #1 ($a) could not be
passed by reference`, raised as the argument is sent (the arguments after it
never run), and a call result is the `Only variables should be passed by
reference` notice. A by-reference result is now written back from a method or
static call with a spread or named arguments after it, and from a named
argument to a user function (`f(out: $r)`), whose unset variable is read
quietly as the positional form's is.

**Smaller fixes.** Unpacking a scalar literal inside a constant array literal
(`[1, ..."ab"]`) is the compile-time fatal it is in the reference, before any
output and uncatchable. `|`, `&` and `^` put a constant left operand second, as
`*` already did, which is visible in `Unsupported operand types: array | string`.
A `<<<` that does not open a well-formed heredoc header lexes as `<<`, so the
parse error names that token. `substr_count()` refuses an offset or length that
leaves the haystack (`ValueError`) instead of clamping it.

---

## Round 16 — `final` enforced

Measured under `PHP 8.5.11 (cli) (built: Sep 22 2026 13:32:06) (NTS)`; ini state
and environment otherwise as recorded in the oracle table above. Every
expectation below was byte-diffed against that binary (stdout, stderr and exit
status).

**`final` was parsed and ignored.** A `final` class could be extended and a
`final` method overridden. The link-time checks of `zend_do_inheritance` are
now made, in its order — a final parent class, then properties, constants and
methods, each in the order the ancestor declared them — with the reference's
messages: `Class B cannot extend final class A`, `Cannot override final
property A::$p`, `B::X cannot override final constant A::X` (an interface's
final constant included, however far up it was declared) and `Cannot override
final method A::f()` (spelled as the overriding method spells it, and reported
at that method's line). The nearest ancestor declaring a member decides, and
is the class named. A final method taken from a trait belongs to the class
that uses it; a private method's `final` binds only on the constructor.

An early-bound class links before the file runs, so the fatal comes before any
output; any other class links where its declaration runs, after whatever ran
first and with the stack trace of the frame declaring it. The prelude's
`Exception`/`Error` getters are now `final`, as the engine's are, so
`class E extends Exception { function getMessage() {} }` is the reference's
`Cannot override final method Exception::getMessage()`.

---

## Round 15 — declaration-time diagnostics, `&` in array literals, `error_get_last`

Measured under `PHP 8.5.11 (cli) (built: Sep 22 2026 13:32:06) (NTS)`; ini state
and environment otherwise as recorded in the oracle table above. Every
expectation below was byte-diffed against that binary.

**Redeclaring a function was not diagnosed.** A top-level function declared
twice, or named like a library function, is now the compile-time `Cannot
redeclare function f()` fatal, so the program prints nothing. A function or
class declared inside an `if`, a loop or another function's body is bound when
its statement runs, so `function_exists`/`class_exists` answer false before then
and a `function_exists`-guarded polyfill no longer replaces the library
function. A type clash is reported where the reference binds it, naming the
type already declared. Pinned in `tests/redeclaration.rs`.

**Class declarations were not checked.** A repeated or misplaced modifier
(`final final class`, `abstract final class`, `public public $x`,
`static const`, `readonly function`, `abstract final function`) is the
grammar's `Fatal error` without a stack trace. A constant, enum case, property
or method declared twice, an abstract method in a class not declared abstract,
an abstract method with a body or a concrete one without, a non-public
interface method, an untyped or static `readonly` property, a promoted
parameter that repeats a property, and a parameter named twice are the
compile-time `Fatal error` with its trace. Pinned in
`tests/class_member_diagnostics.rs`.

**`&` in an array literal was refused.** `[&$a]` and `['k' => &$x]` now bind
the element to the variable, through the same path as `$t[] = &$x`.

**`error_get_last()` and `error_clear_last()` were missing.** Every diagnostic
records its level, message, file and line before the `error_reporting` and `@`
checks, as the reference does, so `@f(); error_get_last()` reads a suppressed
failure.

---

## Round 14 — include/require/eval, stream resources, and the argument checks around them

Measured under `PHP 8.5.11 (cli) (built: Sep 22 2026 13:32:06) (NTS)`; ini state
and environment otherwise as recorded in the oracle table above. Every
expectation below was byte-diffed against that binary.

**`include`, `require`, `include_once`, `require_once` and `eval()` did not
parse.** They were reserved words and nothing more, so no program of more than
one file could run. Each now compiles its file or string when it runs and
executes it in the CURRENT frame. The value is the code's `return` value, `1`
(null for `eval`), `true` for a repeated `_once` (the main script counts), or
`false` after the reference's `Failed to open stream` and `Failed opening …
for inclusion` warnings; a failed `require` throws `Error`. A syntax error in
loaded code throws `ParseError` (new, with `CompileError`), displayed as `Parse
error:` when uncaught. Loaded code addresses variables by name, so a scope that
includes or evals keeps its locals out of frame slots; it continues the loaded
program's temporary, `static`, anonymous-class and `try` numbering, so a
`foreach` in a file included inside a `foreach` no longer shares its iterator;
and it knows the by-reference signatures of the functions already loaded. Every
chunk it produces carries its file, so diagnostics, `getFile()`, `__FILE__`,
`__DIR__` and traces name it, and each include or eval is a trace frame of its
own (`#1 main.php(16): include('/path/to/f...')`, `#1 …: eval()`).
`get_included_files()`/`get_required_files()` list what was loaded, and
`get_include_path()` answers `.` (see BUGS.md for the one divergence there).

**Traits.** A trait that uses no other trait is bound before the program runs,
so a class may use one declared further down the file; one that uses traits is
declared where it stands, as the reference does. A class in an included file
may use a trait the includer declared.

**Stream resources were arrays.** `fopen` returned a heap handle that
`var_dump` showed as `array(0) {}`, `echo` as `Array` and `gettype` of a closed
one as `object`. A stream is now numbered from a counter that never reuses a
number — `STDIN`/`STDOUT`/`STDERR` (new) are 1–3, the first stream a script
opens is 5, `php://temp` takes two numbers and a spill past `maxmemory` a
third, and every successful open by `file_get_contents`/`file`/`readfile`/
`file_put_contents`/`include` spends one — and renders as the reference renders
it: `resource(5) of type (stream)`, `Resource id #5`, `(int)`, `gettype`,
`get_debug_type`, `serialize` (`i:0;`), trace arguments, array keys (with the
`used as offset` warning) and comparisons, which compare the number. A closed
stream stays a resource of type `Unknown`. New: `php://memory`, `php://temp`
(`/maxmemory:N`), `php://stdin`/`stdout`/`stderr`/`output`, `tmpfile`, `fgetc`,
`fpassthru`, `ftruncate`, `fstat`, `get_resource_id`, `SEEK_SET`/`CUR`/`END`,
`LOCK_NB`, and `stream_get_contents`' length and offset. `STDIN` is read lazily,
a line at a time for `fgets`. `feof` answers whether a read came up short, not
whether the cursor is at the end; `fseek` may pass the end and a write there
zero-fills; `a` modes append while `ftell` counts from 0; a stream opened for one
direction refuses the other with the `errno=9 Bad file descriptor` notice.
`fopen` and the whole-file functions warn `Failed to open stream: …`.

**Argument checks.** The stream family's first argument is checked as
`PHP_Z_PARAM_STREAM` checks it — `must be of type resource, X given`, or `must
be an open stream resource` at its own position, before later arguments. The
by-reference array of the sort family, `usort`/`uasort`/`uksort`, `array_walk`,
`end`/`reset`/`next`/`prev` and `array_push`/`pop`/`shift`/`unshift`/`splice`
was never checked, and the latter auto-vivified an unset variable: an unset
variable now binds as null without a warning and is refused with the
reference's `TypeError`, like any other non-array. `fread`/`fgets` with a
length below 1 and an empty `fopen` path are `ValueError`s.

**Two display fixes.** An operand an operator refuses (`1 + []`) threw from line
0 when no call had stamped the frame's line yet. And an uncaught argument-type
`TypeError` from a user function now reads `…, called in X on line N and
defined in Y:M`, the way `Exception::__toString` extends it.

---

## Round 13 — computed member names, uninitialized typed properties, and the rest of ext/json

Measured under `PHP 8.5.11 (cli) (built: Sep 22 2026 13:32:06) (NTS)`; ini state
and environment otherwise as recorded in the oracle table above. Every
expectation below was byte-diffed against that binary.

**`$o->$name` and `$o->{expr}` did not parse.** Only a bare identifier could
follow `->`, so both forms were `Parse error: unexpected variable` / `unexpected
token "{"`. The member after `->` and `?->` is now an identifier, a simple
variable or a braced expression (a braced string literal is the literal name),
and every access takes it: read, write, compound assignment, `++`/`--`, `&`,
`isset`, `unset`, array writes through the property, method calls, first-class
callables and the nullsafe chain. The name is evaluated once, after the receiver
and before the value — `$o->{f()} .= "x"` calls `f` a single time — and a `?->`
short-circuit skips it.

**A typed property with no default read as `NULL`.** It now starts
uninitialized, as in the reference: absent from `foreach`, `print_r`,
`var_export`, `json_encode`, `serialize`, `(array)` and `get_object_vars`;
listed by `var_dump` as `uninitialized(T)` in its declared slot but not counted
in the header; and `Error: Typed property C::$p must not be accessed before
initialization` on a read or `++` before the first write. `__get` answers for
one only after an explicit `unset()`. The type prints the way
`zend_type_to_string` spells it (class names first, builtins in the engine's
fixed order, `?T`, `self` resolved, `iterable` as `Traversable|array`).
Two slot-order bugs went with it: a declared property written back after
`unset()` was appended after the dynamic ones, and a trait's properties were
laid out before the class's own.

**ext/json flags and codes.** `JSON_PARTIAL_OUTPUT_ON_ERROR` (substitute in
place — `null` for a cycle or a resource, `0` for a non-finite float or a pure
enum case — and report the last error), `JSON_PRESERVE_ZERO_FRACTION`,
`JSON_UNESCAPED_LINE_TERMINATORS` and `JSON_BIGINT_AS_STRING` are honoured; a
stream resource is `JSON_ERROR_UNSUPPORTED_TYPE` and a NUL-led `stdClass`
property name `JSON_ERROR_INVALID_PROPERTY_NAME`; `JSON_ERROR_RECURSION` through
`JSON_ERROR_NON_BACKED_ENUM` are seeded constants.

Carried from the interrupted previous round: `new` on a trait raises `Cannot
instantiate trait`, static properties enforce their visibility, and class
constants enforce `private`/`protected` with initializers running in the
declaring class's scope.

---

## Round 12 — what a by-reference parameter refuses, and what a trace already converted

Measured under `PHP 8.5.10 (cli) (built: Aug 25 2026 21:09:32) (NTS)`; ini state
and environment as recorded in the oracle table above.

The previous round landed `src/argsig.rs` — the declared arity and parameter
names of every builtin — and was cut off before it could measure anything. That
measurement is the first half of this round, on 12,000 seeds with the corpus
held fixed and only the binary swapped: **193 divergences in 144 gap classes
before the signature table, 71 in 30 after**, 0 skipped, and every seed that
diverged after also diverged before. Each of the 71 replays byte-identically
from the seed it reports (`--once --seed S`), and `--mode NAME --count 600`
generates 600 cases of that mode and compares 600 of them.

The cost of that table was also never measured, and it is real. On a loop
saturated with builtin calls (instructions retired, three reps, A/A spread
0.3%): 8.243e9 before, 9.979e9 after at 40,000 iterations; 24.453e9 and
29.676e9 at 120,000. The deltas are 1.736e9 and 5.223e9 for an exactly 3×
workload — a ratio of 3.009, so the whole of it is per-call and none of it is
fixed startup. That is +21% on a builtin-saturated loop in a debug build, where
a table lookup pays for every uninlined string comparison twice over.

The fixes this round are the three gaps the 12,000-seed sweep then reported, all
in how a call judges its arguments.

**A user function's `&$a` bound whatever it was given.** `function f(&$a) {}
f(1)` ran, where the reference refuses the call: `Error: f(): Argument #1 ($a)
could not be passed by reference`. The by-reference BUILTINS already had the
three groups — a location binds silently, a call's temporary binds with
`Notice: Only variables should be passed by reference`, a literal or constant or
operator result is the Error — so the pre-pass that records each user function's
by-reference positions now records their names too, and the existing
`BYREF_ARG_DIAG` path renders the verdict. The message carries the DECLARED
spelling: `function Foo(&$a)` called as `FOO(1)` is `Foo()`.

Two orderings around it were measured rather than assumed. A named argument that
binds NOWHERE is reported before a by-reference refusal written after it, because
the reference sends arguments in written order: `f(b: 2, a: 1)` is `Unknown named
parameter $b` while `f(a: 1, b: 2)` is the by-reference Error. And a named
argument that lands in a VARIADIC by-reference tail is refused as `Argument #1`
however many arguments precede it and whichever of the names is the offender.

An argument written before a `...` spread lands in a known position, so it is
judged there too — and the by-reference WRITE-BACK follows it: `f($q, ...[2, 3])`
on `function f(&$a)` left `$q` at its old value while `f($q)` updated it.

**A stack trace showed a builtin's arguments as written.** The reference's
argument parser converts in the argument SLOT for two of the types, so the frame
shows the converted value: `str_pad(5, [])` reports `str_pad('5', Array)`,
`explode(null, [], [])` reports `explode('', Array, Array)`, and
`number_format("1.5", [])` reports `number_format(1.5, Array)`. A plain `int`,
`float` or `bool` parameter converts into a local instead and leaves the slot
alone (`substr('abc', '1', Array)`, `array_chunk(Array, 0, NULL)`), and so does
the `array|string` union (`implode(5, 'x')`). An ARITY refusal precedes all of it
and reports its arguments untouched (`strtolower(5, 6)`). Each rule was probed
per type, because it is a property of the parser function rather than of the
type system.

That conversion is computed only where a trace is actually built. A library
function that runs a PHP callback pushes its frame on the hot path, and none of
those sixteen functions declares a parameter that converts — which is now a unit
test rather than an assumption.

**`count(true)` said `bool given`**, where every PHP says `true given`: the
message reached for `get_debug_type` instead of the value namer they are kept
apart for.

After: **71 divergences in 30 classes → 61 in 20**, same 12,000 seeds, same
oracle, 0 skipped, no seed clean before and diverging after. The builtin-call
workload is unchanged by this round (+0.12%, inside the A/A spread) and the
callback workload is +0.08%, also inside it.

What the sweep still reports, and why each is out of this round's scope: a
generator's `finally` on abandonment (18 of the 61) needs object destruction and
refcounting, the same absent substrate as `__destruct`; a parse error names the
unexpected token but not the expected one (10); `[1, ..."foo"]` is a
COMPILE-time constant-expression fatal in the reference, uncatchable and raised
even in dead code, where phplang throws a catchable `Error` at run time (10); a
clone reports a different object handle, and a closure's clone is a distinct
object rather than a shared one (14); and a by-reference refusal on a METHOD
(`$o->f(1)`) needs the check at run time, where the callee is known (5).

---

## Round 11 — a closure is an object, and `endif` is a keyword

Measured under `PHP 8.5.10 (cli) (built: Aug 25 2026 21:09:32) (NTS)`; ini state
and environment as recorded in the oracle table above.

Round 10 found its bugs by grepping the fuzzer's generated corpus for syntax no
mode emits. Repeating that test found `endif`/`endwhile`/`endfor` at zero hits
— the entire alternative control-structure spelling, which every PHP template is
written in, was a parse error — along with `clone`, `__call`, `get_class`,
`is_a(`, `class_exists` and `func_get_args`. Seven modes were added for those
families and the fixes written against what they reported.

Sampling the corpus needed fixing first. The mode comes from `seed >> 7`, so
seeds `1..6000` reach only the first 47 of the 82 modes: a survey over a flat
seed range reports every later mode's constructs as absent, including the ones
round 10 had just added. The README now documents the per-mode sweep.

### The alternative control-structure syntax is parsed

`if (…): … endif;` and every sibling — `endwhile`, `endfor`, `endforeach`,
`endswitch`, `enddeclare` — were `Parse error: syntax error, unexpected token
":"`. All six now share one `control_body` reader: a `:` after the head opens
the alternative form and its statements run to the terminator, which is left
unconsumed because `elseif`/`else` continue the same `if` and only the caller
knows which of them it accepts. The `;` after the terminator is required exactly
as it is after any statement, so a closing `?>` stands in for it and a body split
by `?> html <?php` still closes.

### `print` is an operator

`print` was read only at the head of a statement, so `$r = print "x"`,
`var_dump(print $v)` and `false or print "y"` were parse errors, and
`true ? print("y") : print("n")` was `Call to undefined function print()`. It is
now an expression that writes its operand like `echo` and evaluates to the int
`1`, read where `yield` is — looser than `=`, tighter than `yield` — so
`print $a = 7` prints the assignment. The statement form is the same production
with its value discarded.

### `and`, `or` and `xor` sit below assignment

`and` and `or` were folded onto `&&` and `||`, which binds them TIGHTER than
`=` — the opposite of the one property that makes the word spellings worth
having. `$x = true and false` assigned `false` where the reference assigns
`true`, and `$a = 1 and 2` assigned `bool(true)` where the reference assigns
`int(1)`. `xor` was not read at all: `true xor false` was
`syntax error, unexpected token "xor"`.

The three now form their own precedence layer between `expression()` and
`assignment()`, loosest first `or` < `xor` < `and`. `xor` has no short circuit
and yields a bool, so it lowers to `(bool)a !== (bool)b` with both sides
evaluated left to right — an exact desugaring that needs no opcode.

### Closures and generators are objects to the reflection surface

```text
$ php -r 'function g(){ yield 1; } var_dump(get_class(g()));'
string(9) "Generator"
```

phplang answered `TypeError: get_class(): Argument #1 ($object) must be of type
object, object given` — a message that argues with itself, because the value
renders as `object` and fails the `is_object` test at the same time. A closure
and a generator are `PhpObj` variants rather than class instances, and each
predicate either special-cased `is_closure` by hand or missed the case
entirely.

`PhpHost::instance_class` now answers "what class does PHP think this value is"
in one place — a class instance's class, `Closure`, or `Generator` — and the
whole surface goes through it: `is_object`, `gettype`, `get_debug_type`,
`get_class`, `::class`, `instanceof`, `is_a`, `is_subclass_of`,
`method_exists`, `get_object_vars`, `get_class_methods`, the stack-trace
argument renderer, the `Unsupported operand types` name and the
`must be of type X, Y given` name. Fourteen answers changed; the three
hand-written `is_closure` special cases went away with them.

The ancestry those predicates walk lives in a `BUILTIN_TYPES` table, because
nothing in the class table says a `Generator` implements `Iterator`. It carries
the two engine classes and the ten engine interfaces (`Traversable`, `Iterator`,
`IteratorAggregate`, `Countable`, `ArrayAccess`, `Stringable`,
`JsonSerializable`, `Throwable`, `UnitEnum`, `BackedEnum`), each with its
parents and its method names, so `$gen instanceof Traversable`,
`method_exists($gen, "current")` and `class_implements("Generator")` all answer.
Engine classes this runtime does NOT implement — `WeakMap`, `Attribute`,
`Fiber` — are deliberately absent: answering `class_exists` yes for a name `new`
cannot build trades one wrong answer for a worse one.

`Closure` is also the one class the reference gives an identity-only `==`: two
closures built from the same literal are not equal, and neither is a rebound
copy. A `Generator` takes the ordinary property walk, has none, and so is `==`
every other generator.

### Each existence predicate answers for its own kind

`interface_exists`, `trait_exists` and `enum_exists` returned `false`
unconditionally, and `class_exists` returned true for every declared name
whatever its kind. In the reference each answers for exactly ONE kind:
`interface I {}` makes `interface_exists('I')` true and `class_exists('I')`
false. `ClassDef` now records `is_trait` and the `use` list it composes, and
`PhpHost::type_kind` reports the kind; an enum is the one name that is two
things, a class and an enum, as the reference has it.

`class_implements` and `class_uses` returned an empty array for every class, on
the grounds — recorded in the module docs — that "this runtime has no
interfaces" and "no traits". Both were stale: interfaces have been in `ClassDef`
for rounds. They now report the real transitive interface list and the real
`use` list, and all three of `class_parents`/`class_implements`/`class_uses`
raise the reference's `Class X does not exist and could not be loaded` warning
before answering `false`.

### `isset()` on anything but a variable is a compile-time fatal

```text
$ php -r 'function f(){ echo "F"; return 1; } var_dump(isset(f()));'
Fatal error: Cannot use isset() on the result of an expression (you can use "null !== expression" instead)
```

phplang evaluated the call and answered `bool(true)` — a wrong answer AND
output the reference never produces, because the rejection happens at compile
time and nothing runs. `isset()` now accepts only a variable, a variable
variable, an index, a property (arrow or nullsafe) and a static property, judged
on the OUTERMOST node alone: `isset(f()->p)` and `isset(f()[0])` stay legal,
`isset(C::K)`, `isset(NAME)`, `isset(new C)` and `isset(1 + 1)` do not.

### An array or an object cannot be an offset

`$a[[1]]` warned `Array to string conversion` and looked the element up under
the key `"Array"`. The reference has no coercion here at all: it is a
`TypeError`, worded `Cannot access offset of type array on array` in a read or a
write and `… in isset or empty` under `isset()`/`empty()` — which is why
`empty($a[k])` now lowers to its own `INDEX_GET_EMPTY` rather than sharing the
`??` opcode. An object names its class (`… of type stdClass`, `… of type
Closure`). An `ArrayAccess` receiver is exempt, because `offsetGet` takes any
key.

### A receiver is judged before the arguments run

```text
$ php -r 'function f(){ echo "F"; return 1; } $n = null; $n->m(f());'
Fatal error: Uncaught Error: Call to a member function m() on null
```

Nothing `f` echoes appears: the reference decides the receiver cannot take a
call before it evaluates one argument. phplang printed `F` first. A method call
that HAS arguments now emits `MCALL_RECV_CHECK` between the receiver and them.
The op looks nothing up — the test is on the value's own shape — so a call that
is going to succeed pays one discriminant check and no second method
resolution, and a zero-argument call, which has nothing to observe, emits
nothing at all.

Whether the method EXISTS is still decided after the arguments; closing that
needs the resolution carried forward to the call so it is not paid for twice,
and is left for a later round. The `callorder` mode scores it.

### An enum case refuses to be cloned

`clone E::A` produced a second object, so `E::A === clone E::A` was false and an
enum stopped being a singleton. The reference answers
`Error: Trying to clone an uncloneable object of class E`, the same refusal a
generator gets.

### A variable read only through `::` was invisible to the frame-slot analysis

```text
$ php -r 'class C {} $o = new C; try { echo $o::class; } catch (\Throwable $e) { echo "E"; }'
C
```

phplang answered `TypeError: Cannot use "::class" on null`. Three separate
passes in `promote.rs` matched `Expr::StaticGet(..)`, `Expr::StaticProp(..)` and
`Expr::StaticCall(_, _, args)` and never looked at the `ClassRef`, which may
hold an expression. A variable reached ONLY through `$v::` therefore looked
unread, was promoted into a frame slot, and read back as null from any detached
chunk — a `try` body most visibly, since that runs on the enclosing scope by
name. All three now walk `ClassRef::operand()`.

### `method_exists` rejects a subject that is neither object nor string

Its parameter is declared `object|string`, so `method_exists(null, 'x')` is a
`TypeError` in the reference; phplang answered `false`.

### Harness

Seven modes, each for a family the corpus grep showed at zero: `altsyntax`
(the six alternative spellings, including bodies split by `?> html <?php`),
`printexpr` (`print` in expression position and the word logical operators),
`reflect` (the predicates above over closures, generators, enum cases,
interfaces and traits), `issetform` (every operand shape `isset`/`empty` can be
given), `callorder` (a side effect in an argument of a call that is going to
fail), `cloning` (`clone`, `__clone`, readonly and the uncloneable values) and
`magiccall` (`__call`/`__callStatic`/`__invoke`).

`altsyntax`, `printexpr`, `reflect` and `issetform` run clean at 800 cases each.
`callorder`, `cloning` and `magiccall` still report, and are left in place
scoring what is not fixed:

* an undefined METHOD or function is still resolved after the arguments run;
* every library function that takes a callback reports an unresolvable one as
  `Error: Call to undefined function`/`method`, where the reference raises a
  catchable `TypeError` naming the parameter — the gap `BUGS.md` already records
  under "Argument type checks are missing on a broad set of library functions",
  now measured through `call_user_func`, `array_map` and `usort` as well;
* a dynamic property on a `Closure` is accepted rather than
  `Error: Cannot create dynamic property Closure::$x`;
* `$obj->undefined->k = 1` warns and continues where the reference raises
  `Error: Attempt to assign property "k" on null`;
* the object-ordinal divergence `var_dump` prints as `#N`, which `BUGS.md` and
  `PhpHost::object_ordinal` already record as needing refcounted handles.

Two items deferred by round 10 are deferred again, for the reason recorded
there: object DESTRUCTION — `__destruct`, and a generator's `finally` running
when the generator is abandoned — needs refcounting the append-only object arena
does not have.

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
