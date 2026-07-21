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
  comparisons, and everything PHP-specific lower to `CallBuiltin` handlers.

## [0x02] USAGE

```sh
php script.php              # run a file
php -r 'echo 1 + 1;'        # run a one-liner (no <?php tag needed)
php -a                      # interactive REPL (persistent state per line)
php --dump-bytecode f.php   # print the lowered fusevm bytecode
```

A `man/php.1` man page and runnable `examples/*.php` ship with the crate.

## [0x03] SUPPORTED TODAY

A working core, grown outward from the sibling frontends. Implemented and tested
end-to-end (see `tests/basic.rs`):

- `<?php … ?>` tags with inline-HTML passthrough; `<?=` short echo; `#`, `//`,
  and `/* */` comments.
- Scalars, single- and double-quoted strings with `$var` interpolation, escapes.
- Variables, arithmetic (`+ - * / % **`), string concat (`.`), compound
  assignment (`+= .=` …), pre/post `++`/`--`.
- Loose/strict comparison (`== != === !== < > <= >=`, PHP-8 string↔number
  ordering), short-circuit `&& || and or`, ternary `?:` (incl. the elvis short
  form), null-coalesce `??`, `!`, and `(int)`/`(float)`/`(string)`/`(bool)` casts.
- Indexed, associative, and appended (`$a[] =`) arrays; index read/write; deep
  and nested lvalues (`$a[b][c] =`, `$a[b][] =`, compound and `++`/`--` on
  elements); the by-reference array mutators (`array_push`/`pop`/`shift`/
  `unshift`/`splice`).
- `if` / `elseif` / `else`, `while`, `do … while`, `for`,
  `foreach ($a as [$k =>] $v)`, `switch` (with fall-through), `break`,
  `continue`, `return`; `match` expressions (a no-arm/no-`default` match throws
  `\UnhandledMatchError`, as PHP 8 does).
- User `function`s with positional, default (`$x = 1`) and variadic (`...$rest`)
  parameters, recursion, and call-site argument unpacking (`f(...$args)`);
  anonymous `function () use (...) { … }` closures and `fn (…) => …` arrow
  functions as first-class callables (`$f(...)`).
- Classes/OOP: `new`, instance properties and methods, `$this`, constructors
  (with property promotion), class constants, `::class`, static methods/constants,
  `self::`/`parent::`, single inheritance, **interfaces** (`implements`, interface
  `extends`), the **`instanceof`** operator, and **traits** (`use Trait;` member
  merging). References — `$b = &$a`, `foreach ($a as &$v)`, and by-reference
  parameters (`function f(&$x)`). Namespaces are accepted in a flat model
  (`namespace X;` / `use A\B\C;`; qualified names fold to their short name).
- Exceptions: `throw` as a statement and a PHP-8 expression (`$x ?? throw …`),
  `try` / `catch (A | B $e)` / `finally` with `finally`-always semantics (it runs
  on return, throw, break, and continue out of the guarded body), a built-in
  exception hierarchy (`Exception`/`Error` disjoint roots under `Throwable`, plus
  `RuntimeException`, `LogicException`, `InvalidArgumentException`, `TypeError`,
  `ValueError`, `UnhandledMatchError`, `DivisionByZeroError`) that user classes
  can subclass, and `getMessage()`/`getCode()`/`__toString()`.
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
    `quote`/`grep` (byte-mode by default, Unicode with `/u`; PCRE subset — no
    backreferences/lookaround).
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
with `as` alias remapping. **Generators (`yield`)** are blocked on the shared VM: phplang
runs each function to completion on a fresh `fusevm` VM, and `fusevm` exposes no
frame suspend/resume primitive, so a faithful lazy generator cannot be built in the
frontend alone (it needs VM-level support). Closures and arrow functions do not yet
capture by reference (`use (&$v)` is rejected). A few current deviations, documented
in-code: array semantics are reference-based rather than PHP's copy-on-write; loose
comparison follows a simplified model; default parameter values are not restricted to
constant expressions; functions with a by-reference OUT parameter (`parse_str`,
`preg_match`'s `$matches`) return the value instead. Persistent bytecode caching and AOT (`--build`) —
present in the sibling frontends — are not wired yet; an LSP server (`--lsp`) and
a DAP debug adapter (`--dap`, with source-line and function breakpoints, stepping,
call stack, locals, and expression `evaluate`) are.

## [0x05] PARITY FUZZER

`parity-fuzz` is a differential fuzzer: it generates seed-deterministic PHP
snippets, runs each through both the reference `php` and phplang, and reports
every case where stdout or success/failure diverges. It is a development tool —
it needs a reference `php` on `PATH`, so CI never runs it.

```sh
cargo build --bin parity-fuzz
./target/debug/parity-fuzz --count 5000        # fuzz 5000 cases
./target/debug/parity-fuzz --once --seed 1234  # replay one case, show both sides
```

Generators are biased toward where a PHP frontend is likely to disagree with the
reference: float formatting, integer division/modulo signs, `**` precedence,
loose-vs-strict comparison, `sort` ordering, `sprintf`/`number_format`, and
string↔number coercion. Divergences are delta-debugged to a minimal reproducer
and grouped by signature; a full report lands in
`target/parity-fuzz/divergences.txt`.

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
