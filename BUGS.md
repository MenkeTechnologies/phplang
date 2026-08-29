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

The `chr()` deprecation for an out-of-range value IS emitted; only the byte
width differs. This is architectural and affects `chr`, `strpbrk` on a
mid-character match, and any other path that would produce a raw byte. Already
recorded in the `chr` corpus entry as a DIVERGENCE.

Further members of this family, measured in round 8 while porting the string
functions around them. All four are the SAME root cause — there is no binary
string type — and none can be fixed inside the function:

```text
$ php -r 'var_dump(quoted_printable_decode("h=C3=A9llo"));'              => string(6) "héllo"
$ target/debug/php -r 'var_dump(quoted_printable_decode("h=C3=A9llo"));' => string(8) "hÃ©llo"

$ php -r 'var_dump(strlen(hex2bin("c3a9")), strlen(base64_decode("w6k=")));'              => int(2) int(2)
$ target/debug/php -r 'var_dump(strlen(hex2bin("c3a9")), strlen(base64_decode("w6k=")));' => int(4) int(4)

$ php -r 'var_dump(strlen(count_chars("aab", 4)));'              => int(254)
$ target/debug/php -r 'var_dump(strlen(count_chars("aab", 4)));' => int(510)
```

`count_chars` modes 3 and 4 are the clearest case: their whole contract is to
name bytes, and 128 of the 256 possible ones widen. Modes 0-2 and every
ASCII-subject call are exact. Closing this needs `Value::Str` to become a byte
string, which is an engine-wide change, not a library one.

---

# Found, reproduced, not yet fixed

Divergences measured but not closed. Each is a concrete next task, not a note.

Entries here accumulate across rounds, so an entry can go stale silently when a
later round fixes what it describes — which is how a page whose whole point is
honesty starts lying. Round 9 re-ran every transcript below against a pristine
build and found four that had already been fixed and were still listed as open
(`min`/`max` with a NaN operand, `array_walk`'s by-reference first argument, the
missing `intdiv` trace frame, and `echo NAN`'s coercion warning). All four are
gone, and the entries they were half of say so. Re-run this section before
trusting it.

## A parse error does not say what was expected

```text
$ php -r '$s = "hello"; var_dump($s{0});'
PHP Parse error:  syntax error, unexpected token "{", expecting ")"
$ target/debug/php -r '… same …'
Parse error: syntax error, unexpected token "{"
```

The token that was found is right; the `, expecting <token>` tail is missing
everywhere. The parser knows what it was about to accept at each of these
sites, so this is threading that through the error, not new analysis.

## `Array to string conversion` is not raised where an array is a KEY

```text
$ php -r 'var_dump(array_unique([1, "1", [1]]));'
PHP Warning:  Array to string conversion
… (the array is otherwise identical)
$ target/debug/php -r '… same …'
… no warning
```

The value is right and the warning is missing. The warning IS raised for the
ordinary coercions — `"x" . [1]`, `"val: $a"`, `(string)[1]` all agree with the
reference — so what is left is the paths that stringify an array to use it as a
comparison or array KEY rather than as a value.

CORRECTED: this entry used to read "Two coercion warnings are not raised" and
listed `echo NAN` alongside. `echo NAN` agrees with the reference and did so
before this round's work; the claim was stale, not fixed here.

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

## A user comparator is called in a different ORDER, and a different number of times

`array_udiff` and `array_uintersect` agree with the reference on the result, and
disagree on the sequence of comparator calls that produced it — visible to any
comparator with a side effect.

```text
$ P='$log=[]; $c=function($x,$y) use (&$log){ $log[]="$x<=>$y"; return $x <=> $y; };
     $r=array_udiff([3,1,2,5],[2,4],$c); print_r($r); echo count($log),"\n",implode(",",$log);'

$ php -r "$P"
Array ( [0] => 3 [1] => 1 [3] => 5 )
12
3<=>1,2<=>1,3<=>2,3<=>5,2<=>4,1<=>2,1<=>2,2<=>2,2<=>3,3<=>4,3<=>5,5<=>4

$ target/debug/php -r "$P"
Array ( [0] => 3 [1] => 1 [3] => 5 )
7
3<=>2,3<=>4,1<=>2,1<=>4,2<=>2,5<=>2,5<=>4
```

The reference sorts both operands with the comparator and then walks them in
step; phplang scans the pool linearly for each probe. The results agree for any
consistent comparator, so closing this is a faithful port of `php_array_diff` /
`php_array_intersect` from `ext/standard/array.c`, not a repair — nothing but the
call log observes it.

## Argument type checks are missing on a broad set of library functions

Sampled, all reproduced; the reference throws and phplang continues:

| call | reference | phplang |
|---|---|---|
| `iterator_to_array(1)` | `TypeError: … must be of type Traversable\|array, int given` | `array(0) {}` |
| `call_user_func_array("strlen", ["a","b"])` | `ArgumentCountError: strlen() expects exactly 1 argument, 2 given` | `int(1)` — the extra argument is dropped |
| `call_user_func("nope")` | `TypeError: … must be a valid callback, function "nope" not found or invalid function name` | `Error: Call to undefined function nope()` — right failure, wrong class |
| `reset($undefined)` | `TypeError: reset(): Argument #1 ($array) must be of type array, null given` | `Warning: Undefined variable`, then `false` |
| `usort($undefined, …)` | `TypeError: usort(): Argument #1 ($array) must be of type array, null given` | `Warning: Undefined variable`, then `true` |
| `array_splice($undefined, 0)` | `TypeError: array_splice(): Argument #1 ($array) must be of type array, null given` | no diagnostic |
| `new ArrayObject(1)` | `TypeError: ArrayObject::__construct(): Argument #1 ($array) must be of type array, int given` | accepted |

This is a systematic gap — `crate::argtypes` covers the names it has entries for
and nothing else — rather than a handful of sites, so it is left for a round that
can widen the table.

CORRECTED: this table used to open with `strlen([])` answering `int(5)`. It now
raises the reference's `TypeError` with the reference's message, and (since this
round) with the reference's frameless trace; the row was stale and has been
removed rather than left to imply the check is missing.

The `call_user_func("nope")` row is not confined to `call_user_func`: every
library function that takes a callback reports an unresolvable one the same wrong
way, `Error: Call to undefined function nope()` where the reference raises a
`TypeError` naming the parameter. `array_map("nosuchfn", [1])` and
`preg_replace_callback("/a/", "nope", "a")` were both measured. Only the CLASS
and message differ — since this round the trace frame is the reference's on all
three.

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
| `json_encode([1.0], JSON_PRESERVE_ZERO_FRACTION)` | `[1.0]` | `Undefined constant` — the flag is not defined |
| `json_decode("12345678901234567890123", false, 512, JSON_BIGINT_AS_STRING)` | `string(23)` | `float(1.2345678901234568E+22)` — the flag is defined but not read |
| `usort($x, ["C", "m"])` for a non-static `C::m` | `TypeError: usort(): Argument #2 ($callback) must be a valid callback, non-static method C::m() cannot be called statically` | the call succeeds |

---

## An array read and mutated in the SAME call sees only its final state

phplang carries a PHP array as a HANDLE, so a by-value argument and a later
by-reference argument in one call end up naming the same array. The reference
copies the value at the moment the argument is evaluated, so it renders the
array as it was BEFORE the mutation that follows it in the same call.

```text
$ php -r '$b=[3,1,2]; var_dump(sort($b), $b, array_pop($b), $b);'
bool(true)
array(3) { [0]=> int(1) [1]=> int(2) [2]=> int(3) }
int(3)
array(2) { [0]=> int(1) [1]=> int(2) }

$ target/debug/php -r '$b=[3,1,2]; var_dump(sort($b), $b, array_pop($b), $b);'
bool(true)
array(2) { [0]=> int(1) [1]=> int(2) }
int(3)
array(2) { [0]=> int(1) [1]=> int(2) }
```

The second `var_dump` argument is the one that differs: the reference shows the
three-element array `sort` had just produced, phplang shows the two-element one
`array_pop` had not yet produced when that argument was evaluated.

Splitting the call fixes it, which is what pins the cause to argument
evaluation rather than to `sort` or `array_pop`:

```text
$ target/debug/php -r '$b=[3,1,2]; var_dump(sort($b)); var_dump($b); var_dump(array_pop($b)); var_dump($b);'
bool(true)
array(3) { [0]=> int(1) [1]=> int(2) [2]=> int(3) }
int(3)
array(2) { [0]=> int(1) [1]=> int(2) }
```

The gap is NARROWER than "phplang does not copy arrays". Everything else in the
value model already matches the reference — assignment copies, a by-value
parameter copies, a nested array copies with its parent, `&$x` aliases, and
`foreach` does not disturb the subject:

```text
$ target/debug/php -r '$a=[1,2]; $b=$a; $b[]=3; var_dump(count($a), count($b));'
int(2)
int(3)
$ target/debug/php -r 'function f($x){ $x[]=9; return count($x); } $c=[1]; var_dump(f($c), count($c));'
int(2)
int(1)
```

What is missing is only the case above: an array that is already on the operand
stack as an ARGUMENT when a later argument mutates it. phplang copies eagerly at
the points that bind a name, and a call argument binds no name, so the handle
travels unguarded.

Closing it means refcounting `PhpObj::Array` and copying on write when the count
is above one — the reference's own model — rather than adding another eager copy
at argument-push time, which would cost a copy on every call that passes an
array and would still be wrong for a by-reference parameter, which needs the
real handle. That is a change to the object model and every mutation site, not a
fix to any one function. Nothing here is specific to `$$name`; a plain variable
shows it identically.
