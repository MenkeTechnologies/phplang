# phplang

PHP as a [fusevm](https://crates.io/crates/fusevm) frontend — the first compiled
standalone PHP runtime. phplang is a pure frontend: it lexes and parses PHP,
lowers it to `fusevm::Chunk` bytecode, and executes on fusevm's bytecode VM +
tracing Cranelift JIT. There is no bespoke interpreter loop — codegen and
execution live in fusevm; phplang supplies the PHP object heap and semantics.

The binary is `php`.

## Pipeline

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

## Usage

```sh
php script.php              # run a file
php -r 'echo 1 + 1;'        # run a one-liner (no <?php tag needed)
php -a                      # interactive REPL (persistent state per line)
php --dump-bytecode f.php   # print the lowered fusevm bytecode
```

A `man/php.1` man page and runnable `examples/*.php` ship with the crate.

## Supported today

This is a working core, grown outward from the sibling frontends. Implemented and
tested end-to-end (see `tests/basic.rs`):

- `<?php … ?>` tags with inline-HTML passthrough; `<?=` short echo; `#`, `//`,
  and `/* */` comments.
- Scalars, single- and double-quoted strings with `$var` interpolation, escapes.
- Variables, arithmetic (`+ - * / % **`), string concat (`.`), compound
  assignment (`+= .=` …), pre/post `++`/`--`.
- Loose/strict comparison (`== != === !== < > <= >=`), short-circuit `&& || and
  or`, ternary `?:` (incl. the elvis short form), null-coalesce `??`, `!`.
- Indexed, associative, and appended (`$a[] =`) arrays; index read/write.
- `if` / `elseif` / `else`, `while`, `do … while`, `for`,
  `foreach ($a as [$k =>] $v)`, `switch` (with fall-through), `break`,
  `continue`, `return`; `match` expressions.
- User `function`s with positional parameters and recursion.
- A ~90-function standard library across strings (`str_*`, `substr`, `trim`,
  `sprintf`, `number_format`, `htmlspecialchars`, `chr`/`ord`, …), arrays
  (`array_map`/`filter`/`reduce`/`merge`/`slice`/`keys`/`values`, `sort` family,
  `in_array`, `array_sum`, `range`, …), math (`abs`/`floor`/`ceil`/`round`/`sqrt`,
  trig, `intdiv`, `fmod`), type/util (`is_*`, `gettype`, `json_encode`,
  `var_dump`, `print_r`, `var_export`).

## Not yet (later waves)

Classes, interfaces, traits, namespaces, closures/arrow functions, references,
`try`/`catch`, generators, superglobals, default/variadic/typed parameters, and
deep (`$a[b][c]`) lvalues. A few current deviations, documented in-code: array
semantics are reference-based rather than PHP's copy-on-write, so the `sort`
family returns a sorted copy instead of sorting in place; `array_pop`/`shift` are
omitted pending a host by-reference delete; `match` with no arm and no `default`
yields null instead of throwing `\UnhandledMatchError`; loose comparison follows
a simplified model. Persistent bytecode caching, AOT (`--build`), LSP, and DAP —
present in the sibling frontends — are not wired yet.

## Build

```sh
cargo build
cargo test
```
