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

## A diagnostic inside a multi-line expression names the statement's line

```text
$ php -r $'$x = [\n 1,\n $u];'
Warning: Undefined variable $u in Command line code on line 3
$ target/debug/php -r $'$x = [\n 1,\n $u];'
Warning: Undefined variable $u in Command line code on line 1
```

The reference gives every opcode the line of the AST node it was compiled
from; phplang's expression nodes carry no line, so every op of a statement
takes the statement's first line. A call written on a later line of the same
statement reports that first line too. Closing it is a line field on the
expression nodes, threaded through the compiler.

## `set_error_handler`: when the handler runs

A diagnostic is handed to the user handler at the next builtin boundary after
it was raised, because the handler is PHP code and the diagnostic is raised
inside the host borrow. For a warning raised by one operation and consumed by
the next — the usual shape — that is the same moment. It differs only for a
library call that warns and then itself produces output before returning: that
output comes first here.

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

Sampled, reproduced; the reference throws and phplang continues:

| call | reference | phplang |
|---|---|---|
| `new ArrayObject(1)` | `TypeError: ArrayObject::__construct(): Argument #1 ($array) must be of type array, int given` | accepted |

This is a systematic gap — `crate::argtypes` covers the names it has entries for
and nothing else — rather than a handful of sites, so it is left for a round that
can widen the table.

CORRECTED: two rows left this table when execution stopped reproducing them.
`call_user_func_array("strlen", ["a","b"])` now raises the reference's
`ArgumentCountError`, because every builtin has a declared arity
(`crate::argsig`); `call_user_func("nope")` now raises the reference's
`TypeError: … must be a valid callback, function "nope" not found or invalid
function name`, because a `callable` parameter is checked against the same
decision tree `is_callable` answers from. Four more left when the `iterator_*`
functions and the by-reference array functions (`reset`, `usort`,
`array_splice` on an undefined variable) gained the reference's checks; the
row left is a MISSING TYPE on a prelude class constructor.

CORRECTED: this table used to open with `strlen([])` answering `int(5)`. It now
raises the reference's `TypeError` with the reference's message, and (since this
round) with the reference's frameless trace; the row was stale and has been
removed rather than left to imply the check is missing.

## Object handles (`#N`, `spl_object_id`) are never reused, and closures take none

```text
$ php -r '$a = new stdClass; unset($a); var_dump(new stdClass);'
object(stdClass)#1 (0) {
$ target/debug/php -r '$a = new stdClass; unset($a); var_dump(new stdClass);'
object(stdClass)#2 (0) {

$ php -r '$f = function() {}; var_dump(new stdClass);'
object(stdClass)#2 (0) {
$ target/debug/php -r '$f = function() {}; var_dump(new stdClass);'
object(stdClass)#1 (0) {
```

The reference frees an object's handle when its refcount reaches zero and
hands the number to the next allocation; phplang's heap is append-only with no
refcounts, so it cannot know when a handle became free. Numbering counts class
instances only. Counting closures and generators too would fix the second case
and break the far more common one where a temporary closure (`array_map(fn…)`)
is freed at once and its number reused, so it is left alone. Both need
refcounted handles.

## An object or generator is destroyed late — at request end, not when its last holder lets go

```text
$ php -r 'function g(){try{yield 1;}finally{echo "F ";}} function f(){ $x=g(); $x->current(); echo "in "; } f(); echo "end\n";'
in F end
$ target/debug/php -r 'function g(){try{yield 1;}finally{echo "F ";}} function f(){ $x=g(); $x->current(); echo "in "; } f(); echo "end\n";'
in end
F

$ php -r 'class D{function __destruct(){echo "d\n";}} $a=new D; unset($a); echo "u\n";'
d
u
$ target/debug/php -r 'class D{function __destruct(){echo "d\n";}} $a=new D; unset($a); echo "u\n";'
u
d
```

The reference runs `__destruct` (and destroys a suspended generator, running
the `finally` around its parked `yield`) when the refcount reaches zero.
phplang has no refcounts. It destroys at two points: a generator when a
`foreach` whose subject expression created it is left, and everything at
request end. The request-end sweep IS the reference's: the globals that alone
hold an object are freed newest first (freeing what only they held), then every
other object in creation order, after the shutdown functions and only when the
run did not end on a fatal error proper. So an object that lives to the end of
the script is destroyed exactly where the reference destroys it; one released
mid-script (`unset`, reassignment, a local going out of scope, a temporary) is
destroyed at the end instead. Destroying it at the release point needs to know
that nothing still holds it, and a promoted local in a running fusevm frame or a
value on a VM stack is invisible to the host, so that is left alone rather than
risk destroying a live object.

## `include`: the search path

```text
$ php -r 'include "nope.php";'
Warning: include(): Failed opening 'nope.php' for inclusion (include_path='.:/opt/homebrew/Cellar/php/8.5.11/share/php/pear') in Command line code on line 1
$ target/debug/php -r 'include "nope.php";'
Warning: include(): Failed opening 'nope.php' for inclusion (include_path='.') in Command line code on line 1
```

The reference's `include_path` names the PEAR directory its build was
configured with; phplang has none and searches `.` and then the including
file's directory. The same value is what `get_include_path()` returns.

## Other measured gaps

| form | reference | phplang |
|---|---|---|
| `new DateTime("not a date")` | `DateMalformedStringException` | no throw |
| `new DateTimeZone("Nowhere/Nothing")` | `DateInvalidTimeZoneException` | class not declared |
| `pack()` / `unpack()` | implemented | `Call to undefined function` |
| `goto end; …; end: echo "done";` | `done` | `Parse error: syntax error, unexpected identifier "end"` |
| `iconv_strlen("héllo")` | `int(5)` | `Call to undefined function iconv_strlen()` |
| `strtotime("2024-03-01 10:00 Europe/Paris")` — any zone identifier whose offset is not fixed | `int(1709283600)` | `false`: there is no tz database, so only `UTC` and its aliases and the `Etc/GMT±N` zones resolve (abbreviations such as `CEST` do) |

## `...` unpacking: what is modelled and what is not

Unpacking works in an array literal and at a call site, over arrays, Generators
and Traversables, and raises the reference's "Only arrays and Traversables can
be unpacked" for anything else — with the class the reference picks, which is
not one class: an array literal says `TypeError` for an object and `Error` for a
scalar or `null`, while an argument list says `TypeError` for both. A spread's
STRING keys are named arguments and its integer keys positional, so
`f(...["b" => 2, "a" => 1])` binds by name rather than by position, and a name
that matches no parameter is now the reference's `Unknown named parameter`
rather than a silent drop. A non-unpackable LITERAL inside a constant array
literal (`[1, ..."ab"]`) is the reference's compile-time fatal. One edge
remains:

- **`[...$a] = $b` reports the wrong text.** The reference refuses a spread in a
  destructuring target with "Spread operator is not supported in assignments";
  this rejects it as "invalid assignment target", through the host-level
  `php: <message>` path that no `catch` intercepts — the general compile-time
  diagnostic divergence recorded above, not a spread-specific one.

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
