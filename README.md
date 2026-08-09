```text
██████╗ ██╗  ██╗██████╗ ██╗      █████╗ ███╗   ██╗ ██████╗
██╔══██╗██║  ██║██╔══██╗██║     ██╔══██╗████╗  ██║██╔════╝
██████╔╝███████║██████╔╝██║     ███████║██╔██╗ ██║██║  ███╗
██╔═══╝ ██╔══██║██╔═══╝ ██║     ██╔══██║██║╚██╗██║██║   ██║
██║     ██║  ██║██║     ███████╗██║  ██║██║ ╚████║╚██████╔╝
╚═╝     ╚═╝  ╚═╝╚═╝     ╚══════╝╚═╝  ╚═╝╚═╝  ╚═══╝ ╚═════╝
```

![Rust](https://img.shields.io/badge/Rust-2021-05d9e8?style=flat-square)
[![Docs](https://img.shields.io/badge/docs-online-blue.svg)](https://menketechnologies.github.io/phplang/)
[![Built on](https://img.shields.io/badge/built%20on-fusevm-8a2be2.svg)](https://github.com/MenkeTechnologies/fusevm)
![status](https://img.shields.io/badge/status-active%20%C2%B7%20in%20development-9b5de5?style=flat-square)
![license](https://img.shields.io/badge/license-MIT-ff2a6d?style=flat-square)

### `[PHP, COMPILED TO BYTECODE — ON A SHARED CRANELIFT JIT]`

> *"Zend compiles PHP to its own opcodes and walks them. phplang lowers PHP to a
> shared machine that other languages already run on, and lets a tracing JIT
> compile the hot loops."*

**phplang** is PHP as a [`fusevm`](https://github.com/MenkeTechnologies/fusevm)
frontend — a lexer/parser and compiler that lowers PHP to `fusevm::Chunk`
bytecode running on fusevm's bytecode VM + tracing Cranelift JIT, over a
`PhpHost` object heap. There is no bespoke interpreter loop: phplang is a pure
front end; execution and codegen live in `fusevm` — the same engine behind
[`zshrs`](https://github.com/MenkeTechnologies/zshrs),
[`strykelang`](https://github.com/MenkeTechnologies/strykelang),
[`awkrs`](https://github.com/MenkeTechnologies/awkrs),
[`pythonrs`](https://github.com/MenkeTechnologies/pythonrs), and
[`rubylang`](https://github.com/MenkeTechnologies/rubylang).

It is, to our knowledge, the first compiled standalone PHP runtime. The binary
is `php`.

### [`Read the Docs`](https://menketechnologies.github.io/phplang/) &middot; [`Engineering Report`](https://menketechnologies.github.io/phplang/report.html) &middot; [`Builtin Reference`](https://menketechnologies.github.io/phplang/reference.html) &middot; [`fusevm`](https://github.com/MenkeTechnologies/fusevm)

---

## Table of Contents

- [\[0x00\] Overview](#0x00-overview)
- [\[0x01\] Pipeline](#0x01-pipeline)
- [\[0x02\] Usage](#0x02-usage)
- [\[0x03\] Supported Today](#0x03-supported-today)
- [\[0x04\] Not Yet (Later Waves)](#0x04-not-yet-later-waves)
- [\[0x05\] Parity Fuzzer](#0x05-parity-fuzzer)
- [\[0x06\] Build](#0x06-build)
- [\[0x07\] Documentation](#0x07-documentation)
- [\[0xFF\] License](#0xff-license)

---

## [0x00] OVERVIEW

phplang keeps PHP the language and throws away Zend's execution model. It lexes
and parses PHP (two-mode: inline-HTML passthrough plus `<?php … ?>` code), lowers
the AST to `fusevm` bytecode, and runs it on the shared bytecode VM with a
tracing Cranelift JIT. Arithmetic lowers to native ops so the JIT can trace hot
loops; PHP-specific behavior — loose comparison, string↔number coercion, array
semantics, the standard library — is served by the `PhpHost` object heap.

It carries no VM or JIT of its own. Bug fixes and JIT improvements in `fusevm`
land once and benefit every hosted frontend at the same time.

## [0x01] PIPELINE

```
source ──▶ lexer ──▶ parser ──▶ compiler ──▶ fusevm::Chunk ──▶ fusevm VM + JIT
              │         │           │                                  │
         two-mode   PHP AST    lower to bytecode              callbacks into PhpHost
        (HTML/PHP)            (native ops + CallBuiltin)      (builtins + numeric hook)
```

- **Scalars** (`int`, `float`, `bool`, `string`, `null`) ride through the VM as
  native `fusevm::Value`s.
- **Arrays** are heap objects in `PhpHost`; they travel as `Value::Obj(u32)`
  handles into that heap.
- **Arithmetic** `+ - *` lowers to native fusevm ops so the JIT can trace hot
  loops; a strict **numeric hook** supplies PHP coercion when an operand is a
  string/array/null or an `i64` op overflows. `/ % **`, concatenation,
  comparisons, and everything PHP-specific lower to `CallBuiltin` handlers. The
  hook is the *sited* form, so it can index the running chunk's line table and
  report a warning against the operator's own line.
- **PHP 8 operand rules** are applied by every operator that reads a number. A
  string with no numeric prefix (`"g"`, `""`, `"   "`, `"INF"`) makes the
  operation a `TypeError: Unsupported operand types: string + int`; one that
  merely trails garbage (`"5g"`) raises `Warning: A non-numeric value
  encountered` and continues with its prefix. Operands resolve left before right
  and before each operator's own checks, so `"g" / 0` is a `TypeError` rather
  than a `DivisionByZeroError`.

## [0x02] USAGE

```sh
php script.php              # run a file
php -r 'echo 1 + 1;'        # run a one-liner (no <?php tag needed)
php -a                      # interactive REPL (persistent state per line)
php --dump-bytecode f.php   # print the lowered fusevm bytecode
php --tiers f.php           # run it, then report which fusevm tiers took it
```

A `man/php.1` man page and runnable `examples/*.php` ship with the crate.

## [0x03] SUPPORTED TODAY

A working core, grown outward from the sibling frontends. Implemented and tested
end-to-end (see `tests/basic.rs`):

- `<?php … ?>` tags with inline-HTML passthrough; `<?=` short echo; `#`, `//`,
  and `/* */` comments.
- Scalars, single- and double-quoted strings with escapes and the full set of
  interpolation forms — `"$v"`, the simple one-level `"$o->p"` / `"$a[key]"` (where
  an unquoted key is a string, not a constant), the complex `"{$expr}"` for any
  expression, and the legacy `"${v}"`.
- Variables, arithmetic (`+ - * / % **`), string concat (`.`), compound
  assignment (`+= .=` …), pre/post `++`/`--`.
- Loose/strict comparison (`== != === !== < > <= >=`, PHP-8 string↔number
  ordering), short-circuit `&& || and or`, ternary `?:` (incl. the elvis short
  form), null-coalesce `??`, the nullsafe operator `?->` (a null receiver
  short-circuits the whole `$a?->b`/`$a?->b()` to null without evaluating the
  member or its arguments), `!`, and `(int)`/`(float)`/`(string)`/`(bool)` casts.
- Indexed, associative, and appended (`$a[] =`) arrays with PHP **value semantics**
  — assigning, passing, returning or storing one hands over a copy (deep through
  nested arrays; an object inside stays a handle), while `$b = &$a` and a `&$x`
  parameter share; index read/write; deep
  and nested lvalues (`$a[b][c] =`, `$a[b][] =`, compound and `++`/`--` on
  elements); the by-reference array mutators (`array_push`/`pop`/`shift`/
  `unshift`/`splice`).
- `if` / `elseif` / `else`, `while`, `do … while`, `for`,
  `foreach ($a as [$k =>] $v)`, `switch` (with fall-through), `break`,
  `continue`, `return`; `match` expressions (a no-arm/no-`default` match throws
  `\UnhandledMatchError`, as PHP 8 does).
- User `function`s with positional, default (`$x = 1`) and variadic (`...$rest`)
  parameters, recursion, call-site argument unpacking (`f(...$args)`), and
  **named arguments** (`f(name: 1, other: 2)`, mixable with positional, order
  independent, extra names collected into a variadic); anonymous
  `function () use (...) { … }` closures — by value, and by reference with
  `use (&$v)`, which binds the closure's name to the enclosing variable itself —
  and `fn (…) => …` arrow functions.
- **First-class callable syntax** (`strlen(...)`, `$obj->method(...)`,
  `Cls::method(...)`, `$callable(...)`) — each yields a `Closure` that forwards
  its arguments to the referenced function/method.
- **Closure rebinding**: `Closure::bind($fn, $obj, $scope)`, `$fn->bindTo($obj,
  $scope)`, and `$fn->call($obj, …)` rebind `$this` and the private/protected
  access scope; a closure created inside a method auto-binds the current `$this`
  and class scope.
- **Generators** (`yield`): a function whose body contains `yield` returns a
  lazy `Generator` — `yield $v`, `yield $k => $v`, bare `yield`, and `yield from`
  (delegating to an array, `Traversable`, or another generator). The `Generator`
  object supports `current`/`key`/`next`/`valid`/`send`/`throw`/`getReturn` and
  drives `foreach` lazily (side effects interleave; infinite generators work).
  Implemented as host-side stackful coroutines (`corosensei`) — the fusevm VM run
  loop executes on the coroutine's stack, so `yield` suspends it with one stack
  switch and no VM change.
- Classes/OOP: `new`, instance properties and methods, `$this`, constructors
  (with property promotion), class constants, `::class`, static methods/constants,
  `self::`/`parent::`, **late static binding** (`static::` — `static::class`,
  `new static`, `static::CONST`, `static::$prop`, `static::m()` — resolves to the
  class the call was made on, and `self::`/`parent::`/`static::` calls forward
  it), single inheritance, **interfaces** (`implements`, interface
  `extends`), the **`instanceof`** operator, and **traits** (`use Trait;` member
  merging). `abstract` classes and interfaces reject direct instantiation
  (`new` on either is a `Cannot instantiate …` error). **References** — `$b = &$a`,
  references to a container slot in either direction (`$r = &$a['x']['y']`,
  `$r = &$o->p`, `$a[] = &$v`, `$o->p = &$v`), return-by-reference
  (`function &f()`), `foreach ($a as &$v)`, and by-reference parameters
  (`function f(&$x)`); a referenced element stays shared across an array copy and
  `var_dump` marks it with `&`, as PHP does.
  Namespaces are accepted in a flat model
  (`namespace X;` / `use A\B\C;`; qualified names fold to their short name).
- **Enums** (PHP 8.1): pure enums (`enum Suit { case Hearts; … }` with
  `Suit::Hearts`, `->name`, `Suit::cases()`, singleton `===` identity) and backed
  enums (`enum Status: string { case Active = 'active'; }` with `->value`,
  `Status::from()`, `Status::tryFrom()`); enums may declare methods and constants
  and satisfy `instanceof UnitEnum`/`BackedEnum`.
- Exceptions: `throw` as a statement and a PHP-8 expression (`$x ?? throw …`),
  `try` / `catch (A | B $e)` / `finally` with `finally`-always semantics (it runs
  on return, throw, break, and continue out of the guarded body), a built-in
  exception hierarchy (`Exception`/`Error` disjoint roots under `Throwable`, plus
  `RuntimeException`, `LogicException`, `InvalidArgumentException`, `TypeError`,
  `ValueError`, `UnhandledMatchError`, `DivisionByZeroError`) that user classes
  can subclass, and `getMessage()`/`getCode()`/`getPrevious()`/`__toString()`.
- **Diagnostics.** `Warning` and `Deprecated` notices go to *stdout*, interleaved
  with the program's output exactly as PHP's CLI defaults put them: undefined
  variables, array keys and properties, array offsets on a non-array, string
  offsets past the end, the `++`/`--` cases that have no effect, and PHP 8.2's
  `Creation of dynamic property C::$p is deprecated`. `isset()`, `empty()` and
  `??` read in PHP's isset mode and stay silent, as do writes, auto-vivification
  and by-reference output arguments. `@` is separate and dynamic: the operand is
  evaluated normally and every diagnostic raised while it runs is dropped,
  including those raised from inside the library functions it calls
  (`@preg_match('/[a', $s)`). Which of them are
  *displayed* is the `error_reporting` mask, writable by `error_reporting()`,
  `ini_set('error_reporting', …)`, or `php -d error_reporting=…`. Only the `-d`
  path runs the php.ini constant-expression scanner, so `E_ALL & ~E_DEPRECATED`
  is understood there and reads as plain integer `0` through `ini_set` — the
  reference behaves the same way.
- **Compile-time notices** are raised while the source is READ, so they precede
  every byte the program writes and fire whether or not the code carrying them
  runs — `Using ${var} in strings is deprecated, use {$var} instead` is the one
  PHP 8.2 added. A run-time `error_reporting(0)` cannot retract one; only the
  startup level (`-d`) applies.
- **Attributes** (`#[Attr]`, `#[Ns\\Attr(1, [2])]`) parse everywhere a
  declaration can carry them — class, function, method, property, class constant,
  enum case, parameter. `#[AllowDynamicProperties]` is honoured, and inherited by
  subclasses.
- **Library argument errors throw.** A standard-library function given arguments
  it rejects raises the catchable exception PHP raises — `ValueError`,
  `DivisionByZeroError` — with the library call itself as frame `#0` of the trace
  (`#0 <file>(<line>): range(9, 10, 2)`), and a `#[\\SensitiveParameter]`
  argument masked as `Object(SensitiveParameterValue)` exactly as PHP masks it.
- **Property overloading** — `__get`, `__set`, `__isset` and `__unset` fire for a
  property the object does not carry, whether because no class declared it,
  because it was `unset`, or because its visibility puts it out of reach of the
  reading scope. They are consulted BEFORE the access error, so a class defining
  them never reports `Cannot access private property`. A magic method is not
  re-entered for the property it is already handling, so `__get($n) { return
  $this->$n; }` terminates. The four questions PHP asks differently are all
  distinguished: `isset()` asks `__isset` alone, `empty()` and `??` follow a true
  `__isset` with `__get` (and `??` falls back to `__get` when there is no
  `__isset`, where `empty()` does not), and a plain read asks `__get`.
- **Property access errors throw.** Reading, writing or unsetting an out-of-reach
  property raises a catchable `Error: Cannot access private property C::$x`
  naming the class that DECLARED it, rather than aborting the process. `isset()`
  never throws — asking is always allowed.
- **`__toString`** is invoked wherever a value becomes a string: `echo`, `print`,
  `.` concatenation, interpolation, the `(string)` cast / `strval`, `implode`'s
  elements, and the library functions whose parameters PHP declares as `string`.
- Integer literals in every base (`0xFF` hex, `0755`/`0o17` octal, `0b101` binary,
  `1_000` separators); predefined constants (`PHP_INT_MAX`, `PHP_EOL`, `M_PI`, the
  `SORT_*`/`FILTER_*`/`JSON_*`/… flag families) plus `define`/`defined`/`constant`;
  and superglobals (`$_SERVER`, `$_ENV`, `$_GET`/`$_POST`/…, `$GLOBALS`, `$argv`/
  `$argc`) auto-global across every scope.
- The `DateTime`/`DateTimeImmutable`/`DateInterval` classes and the SPL data
  structures (`SplStack`, `SplQueue`, `SplDoublyLinkedList`, `SplFixedArray`,
  `ArrayObject`, `SplObjectStorage`, `SplPriorityQueue`, `SplMinHeap`/`SplMaxHeap`)
  plus `stdClass`, all as PHP preludes; output buffering (`ob_start`/`ob_get_clean`/…),
  variadic introspection (`func_get_args`/`func_num_args`), `fopen` file streams
  (`fread`/`fwrite`/`fgets`/`fseek`/`fclose`), the `unset()` construct, `spl_object_id`,
  and the `@` error-suppression operator.
- A large standard library (475+ functions, incl. bcmath and gmp arbitrary precision), split into category modules under
  `src/stdlib/` and consulted through a per-category dispatch chain:
  - **strings** — `str_*`, `substr*`, `strpos`/`stripos`/`strrpos`, `strstr`,
    `strtr`, `sprintf`/`vsprintf`/`sscanf`, `number_format`, `nl2br`,
    `addslashes`, `str_rot13`, `similar_text`, `levenshtein`, `mb_*`, …
  - **arrays** — `array_map`/`filter`/`reduce`/`merge`/`slice`/`column`/`chunk`,
    the `sort`/`usort`/`natsort` families, `array_diff`/`intersect` (+`_key`/
    `_assoc`), `compact`/`extract`, the internal-pointer family, …
  - **math** — `abs`/`floor`/`ceil`/`round`/`sqrt`, full trig + hyperbolic +
    inverse, `hypot`/`fdiv`/`fmod`, base conversions (`dec*`/`*dec`/`base_convert`),
    `rand`/`mt_rand`/`random_int`.
  - **ctype** — the `ctype_*` predicates. **types** — `is_*`, `gettype`,
    `get_debug_type`, `serialize`/`unserialize`, `var_dump`/`print_r`/`var_export`.
  - **preg** — `preg_match`/`match_all`/`replace`/`replace_callback`/`split`/
    `quote`/`grep` (byte-mode by default, Unicode with `/u`). Look-around,
    backreferences, atomic groups and possessive quantifiers all work: a pattern
    the `regex` crate will not compile is retried on `fancy-regex`. `$matches` is
    a real by-reference OUT parameter, so it defines the caller's variable
    whether or not it existed, and a `(?<name>…)` group appears under its name as
    well as its index.
  - **datetime** — `time`/`mktime`/`date`/`gmdate`/`checkdate`/`strtotime` (UTC).
  - **hash** — `md5`/`sha1`/`hash`/`crc32`/`hash_hmac`. **encoding** —
    `base64_*`, `bin2hex`/`hex2bin`, quoted-printable, `utf8_*`. **url** —
    `urlencode`/`rawurlencode` (+decode), `http_build_query`, `parse_url`,
    `parse_str`.
  - **json** — `json_encode`, `json_decode`, `json_last_error`(`_msg`). **filter**
    — `filter_var` (`VALIDATE_INT`/`FLOAT`/`BOOLEAN`/`EMAIL`/`URL`/`IP`/`DOMAIN`/
    `REGEXP`, `SANITIZE_*`). **mbstring** — `mb_str_split`, `mb_convert_case`,
    `mb_strpos`/`rpos`, `mb_ord`/`chr`, `mb_convert_encoding`, `mb_detect_encoding`.
  - **fileio** — `file_get_contents`/`put_contents`, `file`, `fopen`-free file ops
    (`file_exists`, `is_file`/`dir`, `unlink`, `mkdir`, `scandir`, `copy`,
    `basename`/`dirname`/`pathinfo`, `realpath`, `getcwd`, …).
  - **reflection** — `class_exists`, `method_exists`, `property_exists`,
    `get_class`, `get_parent_class`, `get_object_vars`, `get_class_methods`,
    `class_parents`, `is_a`/`is_subclass_of`. **callable** — `call_user_func`(`_array`)
    (incl. array/`Class::method` callables), `function_exists`. **misc** —
    `strnatcmp`/`strnatcasecmp`, `soundex`, `str_getcsv`, `array_walk_recursive`,
    `array_find`/`array_any`/`array_all`, `array_udiff`/`array_multisort`.
  - **system** — `getenv`/`putenv`, `phpversion`, `php_sapi_name`, `php_uname`,
    `getmypid`, `extension_loaded`, `get_defined_constants`, `get_declared_classes`.

## [0x04] NOT YET (LATER WAVES)

Strict typed-parameter enforcement (type hints are parsed but not enforced —
phplang follows PHP's coercive/weak-typing mode) and true (non-flat) namespaces
with `as` alias remapping. A few current deviations, documented
in-code:

- Default parameter values are not restricted to constant expressions.
- The by-reference OUT parameter is implemented for `preg_match`/
  `preg_match_all`/`preg_replace`(`_callback`)/`parse_str`/`similar_text`/
  `str_replace`/`settype` and not for the rest of the library
  (`array_multisort`, `sscanf`'s trailing arguments).
- A diagnostic names the *statement's* line. PHP names the line of the
  expression, so a statement spanning several lines reports its first.
- A `preg_*` pattern the REFERENCE also rejects reproduces its `Warning` and its
  `preg_last_error()` state. One the reference would have compiled but NEITHER
  engine can (pattern recursion, a conditional group) still returns the error
  sentinel SILENTLY — there is no diagnostic to copy. The `D` modifier is
  accepted and ignored; it is right by accident, because both engines' `$` is
  already end-of-haystack only, so the *unmodified* `/a$/` is what differs —
  `preg_match("/a$/", "a\n")` is 1 in the reference and 0 here.
- A pattern that only the `fancy-regex` engine will take matches as if `/u` were
  set, because that engine works over `&str`: `.` is one codepoint rather than
  one byte. This is visible only for a NON-ASCII subject, and only for a pattern
  the byte engine already refused.
- A **stack trace** frame entered from inside a library function (an `array_map`
  callback) prints its call site rather than PHP's `[internal function]`, and a
  closure frame prints `{closure}` rather than PHP 8.4's `{closure:file:line}`.
- A **syntax error** reproduces PHP's `unexpected <token>` text but not the
  `, expecting "X" or "Y"` clause that often follows it: the expected set comes
  out of PHP's generated LALR tables, not the grammar as written here.
- `var_dump`'s `#N` object number and `spl_object_id` agree with each other, but
  PHP reuses a freed object's number and phplang's arena never frees, so the two
  agree only until an object becomes unreachable.
- `ini_get`/`ini_set` know PHP core plus `date` and `pcre` — the two extensions
  PHP 8 cannot be built without — at the values the reference reports for them
  with no php.ini loaded. A name belonging to an optional extension, or one whose
  default is the build's install prefix (`extension_dir`), reads back `false`
  rather than a machine-specific guess. `ini_set` does not model per-setting
  VALUE validation: the reference refuses `ini_set('memory_limit', '20')` with a
  message quoting its live memory usage, which is not a reproducible number.

Persistent bytecode caching and AOT (`--build`) —
present in the sibling frontends — are not wired yet; an LSP server (`--lsp`) and
a DAP debug adapter (`--dap`, with source-line and function breakpoints, stepping,
call stack, locals, and expression `evaluate`) are.

## [0x05] PARITY FUZZER

`parity-fuzz` is a differential fuzzer: it generates seed-deterministic PHP
snippets, runs each through both the reference `php` and phplang, and reports
every case where stdout or success/failure diverges. It is a development tool —
it needs a reference `php` on `PATH`, so CI never runs it. Neither side is run
with `error_reporting` turned down: PHP writes `Warning`/`Deprecated`/`Fatal
error` to stdout, so those are part of the output being compared.

```sh
cargo build --bin parity-fuzz
./target/debug/parity-fuzz --count 5000        # fuzz 5000 cases
./target/debug/parity-fuzz --once --seed 1234  # replay one case, show both sides
```

Generators are biased toward where a PHP frontend is likely to disagree with the
reference: float formatting, integer division/modulo signs, `**` precedence,
loose-vs-strict comparison, `sort` ordering, `sprintf`/`number_format`, and
string↔number coercion, and the PCRE constructs the `regex` crate lacks.
Divergences are delta-debugged to a minimal reproducer and grouped by signature;
a full report lands in `target/parity-fuzz/divergences-<pid>.txt`, named for the
run so concurrent invocations against one checkout cannot overwrite each other.

The exit status answers *did this run measure what it was asked to*, not merely
*did it find a disagreement*. A run exits non-zero when no cases ran, when every
case that ran was skipped (the reference timing out on all of them), when a case
reached a worker and produced no verdict, or when a case agreed only because the
reference printed nothing. Each is named in a closing `RUN NOT CLEAN` line, and
the summary reports the skipped and barren counts even at zero — a clean number
is only evidence if those are visible next to it.

A clean divergence count only means something alongside the two numbers printed
under it. `skipped` counts cases that never reached a comparison — the reference
timed out, or either side failed to run — because a mode whose programs all time
out on the reference would otherwise report zero divergences forever. `barren`
counts cases where both sides agreed only by failing with nothing on stdout:
those ran and matched, but prove nothing about the behaviour they were written
to exercise, and two *different* failures are indistinguishable there. A run is
worth quoting when both are `0`.

## [0x06] BUILD

```sh
cargo build
cargo test
```

phplang is a standalone crate (an explicit empty `[workspace]` stops cargo
walking up to the meta parent). `fusevm` is pulled from crates.io with the `jit`
feature.

## [0x07] DOCUMENTATION

- **Docs hub** — <https://menketechnologies.github.io/phplang/>
- **Builtin reference** — <https://menketechnologies.github.io/phplang/reference.html>
- **Engineering report** — <https://menketechnologies.github.io/phplang/report.html>
- **fusevm** — <https://github.com/MenkeTechnologies/fusevm> (the shared VM)
- **Source** — <https://github.com/MenkeTechnologies/phplang>

## [0xFF] LICENSE

MIT — free and open source. See [LICENSE](LICENSE).
