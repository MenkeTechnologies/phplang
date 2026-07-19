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

## Supported today

This is a working core, grown outward from the sibling frontends. Implemented and
tested end-to-end (see `tests/basic.rs`):

- `<?php … ?>` tags with inline-HTML passthrough; `<?=` short echo; `#`, `//`,
  and `/* */` comments.
- Scalars, single- and double-quoted strings with `$var` interpolation, escapes.
- Variables, arithmetic (`+ - * / % **`), string concat (`.`), compound
  assignment (`+= .=` …), pre/post `++`/`--`.
- Loose/strict comparison (`== != === !== < > <= >=`), short-circuit `&& || and
  or`, ternary `?:`, `!`.
- Indexed, associative, and appended (`$a[] =`) arrays; index read/write.
- `if` / `elseif` / `else`, `while`, `for`, `foreach ($a as [$k =>] $v)`,
  `break`, `continue`, `return`.
- User `function`s with positional parameters and recursion.
- A starter standard library: `strlen`, `count`, `str*`/`substr`/`trim`,
  `implode`, `explode`, `in_array`, `array_keys`/`array_values`/`array_push`,
  `range`, `abs`/`floor`/`ceil`/`round`/`sqrt`/`max`/`min`, `is_*`, `gettype`,
  `sprintf`/`printf`, `print_r`, `var_dump`.

## Not yet (later waves)

Classes, interfaces, traits, namespaces, closures/arrow functions, references,
`match`/`switch`, `try`/`catch`, generators, superglobals, default/variadic/typed
parameters, deep (`$a[b][c]`) lvalues, and the full standard library. Array
semantics are currently reference-based rather than PHP's copy-on-write; loose
comparison follows a simplified model. Persistent bytecode caching, AOT
(`--build`), LSP, and DAP — present in the sibling frontends — are not wired yet.

## Build

```sh
cargo build
cargo test
```
