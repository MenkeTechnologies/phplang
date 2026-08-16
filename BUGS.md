# Known divergences

Behaviour that differs from the reference and is **not** fixed, each with the
command that shows it. The oracle and its ini state are recorded at the top of
[CHANGELOG.md](CHANGELOG.md); every transcript below was produced under it.

A divergence earns a place here only if it has been reproduced. Nothing on this
page is inferred.

---

## Declined: needs memory accounting

phplang has no `memory_limit`, so it cannot reproduce a failure whose message
quotes a byte budget. In each of these BOTH engines stop the program; only the
text differs.

```text
$ php -r 'str_pad("a", PHP_INT_MAX);'
PHP Fatal error:  Allowed memory size of 134217728 bytes exhausted (tried to allocate 9223372036854775833 bytes)

$ target/debug/php -r 'str_pad("a", PHP_INT_MAX);'
memory allocation of 9223372036854775806 bytes failed
```

Same shape for `mb_str_pad("x", PHP_INT_MAX)`, `number_format(1.5,
2147483648)`, `array_pad([1], 100000000, 0)`, `sprintf("%2147483646d", 1)` and
`gmp_pow("2", 4294967296)`. The reference's number changes with its
`memory_limit`, so there is nothing stable to port. `str_repeat` is the one
member of this family that PHP reports deterministically — that one IS
implemented (see CHANGELOG.md).

`gmp_pow` with an exponent past `u32` is the only one given a substitute: it
stops with a phplang-worded fatal of the same shape rather than silently
truncating the exponent and returning `1`. The wording is ours and is marked as
such in the source.

## Declined: unbounded user recursion

```text
$ php -r 'function r($n) { return r($n+1); } r(0);'
PHP Fatal error:  Allowed memory size of 134217728 bytes exhausted … #0 Command line code(1): r(915008) …

$ target/debug/php -r 'function r($n) { return r($n+1); } r(0);'
thread 'main' has overflowed its stack
fatal runtime error: stack overflow, aborting
```

Both die. The reference dies at its memory limit with a trace; phplang dies on
the native stack with a Rust message. A call-depth cap would stop the abort but
could only report a message we invented, so it is left alone rather than
fabricated.

## `serialize()` writes `N;` where the reference writes a back-reference

```text
$ php -r '$a=[1]; $a[]=&$a; echo serialize($a);'
a:2:{i:0;i:1;i:1;a:2:{i:0;i:1;i:1;R:3;}}
$ target/debug/php -r '$a=[1]; $a[]=&$a; echo serialize($a);'
a:2:{i:0;i:1;i:1;N;}

$ php -r '$o=new stdClass; $o->s=$o; echo serialize($o);'
O:8:"stdClass":1:{s:1:"s";r:1;}
$ target/debug/php -r '$o=new stdClass; $o->s=$o; echo serialize($o);'
O:8:"stdClass":1:{s:1:"s";N;}
```

The cycle guard added this round stopped the stack overflow; it did not add
back-references. The reference numbers every value as it serializes it and
emits `r:<n>;` (object) or `R:<n>;` (reference) on a repeat. phplang keeps no
such position table. The output is finite and syntactically valid but no longer
round-trips a self-referential structure.

## `json_decode` caps nesting at 1024 whatever `$depth` says

The decoder is recursive descent on the native stack. Measured by bisection:
5000 levels survive on the 8 MiB main thread and 10000 do not; on a 2 MiB
worker thread (what `cargo test` gives a test function) 1536 survive and 2048
do not. The ceiling is therefore 1024 — inside the smaller, and double the 512
the reference defaults to.

```text
$ D='$d = str_repeat("[",2000) . str_repeat("]",2000);
     var_dump(gettype(json_decode($d, true, 999999)), json_last_error());'

$ php -r "$D"
string(5) "array"
int(0)

$ target/debug/php -r "$D"
string(4) "NULL"
int(1)                       # JSON_ERROR_DEPTH
```

Only a program that explicitly raises `$depth` above 1024 AND feeds it a
document nested that deep can observe it; at the reference's own default of 512
the two agree. Removing the cap requires an iterative parser.

## A non-ASCII byte cannot be represented

`Value::Str` is a Rust `String`, so every string is valid UTF-8 and a lone byte
above 0x7F becomes a two-byte codepoint.

```text
$ php -r 'var_dump(chr(-1));'                 => string(1) "\xff"
$ target/debug/php -r 'var_dump(chr(-1));'    => string(2) "ÿ"
```

The `chr()` deprecation for an out-of-range value IS emitted (fixed this round);
only the byte width differs. This is architectural and affects `chr`,
`strpbrk` on a mid-character match, and any other path that would produce a raw
byte. Already recorded in the `chr` corpus entry as a DIVERGENCE.

---

# Found, reproduced, not yet fixed

Divergences this round measured but did not have room to close. Each is a
concrete next task, not a note.

## Visibility is not enforced on constants or static properties

```text
$ php -r 'class C { private const K = 1; } try { echo C::K; } catch (\Throwable $e) { echo get_class($e),"|",$e->getMessage(); }'
Error|Cannot access private constant C::K
$ target/debug/php -r '… same …'
1

$ php -r 'class C { private static $s = 1; } try { echo C::$s; } catch (\Throwable $e) { echo get_class($e),"|",$e->getMessage(); }'
Error|Cannot access private property C::$s
$ target/debug/php -r '… same …'
1
```

Instance properties and methods ARE enforced (`tests/visibility.rs`); constants
and statics are read without a check.

## `new` on a trait succeeds

```text
$ php -r 'trait T {} try { new T; } catch (\Throwable $e) { echo get_class($e),"|",$e->getMessage(); }'
Error|Cannot instantiate trait T
$ target/debug/php -r '… same …'
(no output — an object was constructed)
```

The other three kinds now raise (`interface`, `enum`, `abstract class`).
Traits are not kept in the class table at all, so there is nothing to test
`class_instantiation_error` against.

## `&$a` inside an array literal is refused

```text
$ php -r '$a=[1]; $b=[&$a]; $a[]=$b; print_r($a);'
Array ( [0] => 1 [1] => Array ( [0] => Array *RECURSION* ) )
$ target/debug/php -r '… same …'
php: `&` in an array literal is supported only in a destructuring target, not in a value array
```

A parser gap, and the message is a scaffold error rather than anything PHP
emits.

## `set_error_handler` is a no-op that `function_exists` denies

```text
$ php -r 'set_error_handler(function($n,$s){ echo "LVL=$n MSG=$s\n"; return true; }); $a=[1]; echo $a[9]; echo "CONT\n";'
LVL=2 MSG=Undefined array key 9
CONT
$ target/debug/php -r '… same …'
Warning: Undefined array key 9 in Command line code on line 1
CONT
```

The callback is accepted and discarded — already recorded as a DIVERGENCE in
the `set_error_handler` corpus entry. What is NOT recorded is that
`function_exists("set_error_handler")` answers `false` while the name is
callable, and the same holds for `trigger_error`, `set_exception_handler`,
`restore_error_handler` and `error_get_last`. Two answers to the same question.

Implementing the handler chain would make the `E_*` level of every diagnostic
observable from PHP, which is the strongest available test for the level work
done this round — currently that level can only be probed indirectly, by masking
the bit with `error_reporting()`.

## A library throw's stack frame is missing on some paths

```text
$ php -r 'enum E: int { case A = 1; } E::from(99);'
Fatal error: Uncaught ValueError: 99 is not a valid backing value for enum E in Command line code:1
Stack trace:
#0 Command line code(1): E::from(99)
#1 {main}

$ target/debug/php -r '… same …'
… identical, except:
#0 {main}
```

`throws_bare` deliberately omits the frame (that is what it is for), but
`Enum::from` is a real call and the reference names it. The framed path
(`throw_from_internal`) is only reachable from `call_library`.

Related: a closure frame prints `{closure}` where the reference prints
`{closure:Command line code:1}`.

## Argument type checks are missing on a broad set of library functions

Sampled, all reproduced; the reference throws and phplang continues:

| call | reference | phplang |
|---|---|---|
| `strlen([])` | `TypeError: strlen(): Argument #1 ($string) must be of type string, array given` | `int(5)` — the array stringified to `"Array"` |
| `iterator_to_array(1)` | `TypeError: … must be of type Traversable\|array, int given` | `array(0) {}` |
| `call_user_func_array("strlen", ["a","b"])` | `ArgumentCountError: strlen() expects exactly 1 argument, 2 given` | `int(1)` — the extra argument is dropped |
| `call_user_func("nope")` | `TypeError: … must be a valid callback, function "nope" not found or invalid function name` | `Error: Call to undefined function nope()` — right failure, wrong class |
| `reset($undefined)` | `TypeError: reset(): Argument #1 ($array) must be of type array, null given` | `Warning: Undefined variable`, then `false` |
| `usort($undefined, …)` | `TypeError: usort(): Argument #1 ($array) must be of type array, null given` | `Warning: Undefined variable`, then `true` |
| `array_splice($undefined, 0)` | `TypeError: array_splice(): Argument #1 ($array) must be of type array, null given` | no diagnostic |
| `new ArrayObject(1)` | `TypeError: ArrayObject::__construct(): Argument #1 ($array) must be of type array, int given` | accepted |

This is a systematic gap — there is no shared parameter-declaration layer to
check against — rather than a handful of sites, so it is left for a round that
can build one.

## Other measured gaps

| form | reference | phplang |
|---|---|---|
| `fopen("/nonexistent/dir/x", "r")` | `Warning: fopen(…): Failed to open stream: No such file or directory` | no diagnostic |
| `fread($closed, 1)`, `fclose($closed)` | `TypeError: … must be an open stream resource` | no diagnostic |
| `new DateTime("not a date")` | `DateMalformedStringException` | no throw |
| `new DateTimeZone("Nowhere/Nothing")` | `DateInvalidTimeZoneException` | class not declared |
| `echo (new class { public int $p; })->p` | `Error: Typed property class@anonymous::$p must not be accessed before initialization` | no throw |
| `pack()` / `unpack()` | implemented | `Call to undefined function` |
| `LC_ALL` and the other `LC_*` constants | defined | `Undefined constant` |
| `JSON_PARTIAL_OUTPUT_ON_ERROR` | defined and honoured | `Undefined constant` |
| `goto end; …; end: echo "done";` | `done` | `Parse error: syntax error, unexpected identifier "end"` |
| `$o->{"x y"} = 2;` | property `x y` is written | `Parse error: syntax error, unexpected token "{"` |
| `iconv_strlen("héllo")` | `int(5)` | `Call to undefined function iconv_strlen()` |
| `usort($x, ["C", "m"])` for a non-static `C::m` | `TypeError: usort(): Argument #2 ($callback) must be a valid callback, non-static method C::m() cannot be called statically` | the call succeeds |

---

# Pre-existing documentation inconsistencies

Not behaviour — two corpus entries that contradict each other and were not
introduced by this round's work:

- `JSON_PRETTY_PRINT` is described as "not honoured — the encoder always emits
  the compact form" in its own entry, while the `json_encode` entry lists it
  among the flags that ARE honoured.
- `round()`'s entry says "the PHP 8.4 `$mode` argument is not read", which the
  commit `7b639caea8` ("round()'s `$mode` was decorative") set out to fix.

Both need a measured check and one of the two statements corrected. They are
listed here rather than edited blind, because guessing which side is right is
how a false claim gets published.
