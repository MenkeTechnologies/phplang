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

`pack()` and `unpack()` are in the same family: a packed byte of 0x80 or more
is two bytes here, so `pack("q", -5)` round-trips through `unpack` wrongly while
every byte below 0x80 is exact.

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

## A parse error does not always say what was expected

```text
$ php -r 'class A { function f() {} 1 }'
PHP Parse error:  syntax error, unexpected integer "1", expecting "function"
$ target/debug/php -r '… same …'
Parse error: syntax error, unexpected integer "1"
```

The `, expecting …` tail is printed at the sites whose list was measured: a
stray token after an argument, array element, parameter, `use` item, `echo`
operand, `return`/`break`/`continue` operand, `global`/`static`/`const`/property
declaration, a `for` header, a `catch` header, a missing `{`, `=>`, `=` or `(`,
and the `unset`/`foreach` operand chain. The scanner-level bracket diagnostics
(`Unmatched ')'`, `Unclosed '[' does not match ')'`) are exact. What remains is
the long tail that lives in PHP's LALR tables, not in the grammar: a list appears
only when the construct reduces its operand to a nonterminal before the
punctuation (`f(1 2)`, `echo 1 2;`) and not after a bare `expr` (`if ($a $b)`),
so each further site has to be measured, not derived. `parity-fuzz --mode
parseerr` inserts a stray operand token into valid programs and reports the
sites still missing — for instance a statement keyword where only `"function"`
may start a class member, `list(…)` without its `=`, and the group-`use` forms.

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

## A chunked output handler runs at the next builtin boundary

A write that fills an `ob_start` handler's `$chunk_size` runs the handler in
the reference DURING the write. Here the handler is PHP code and the write
happens inside the host borrow, so it runs when the builtin that wrote returns
(`echo` checks after each of its operands). The two differ only for one library
call that writes several times — a diagnostic followed by the function's own
output — where the reference calls the handler between the writes and this
runs it once on both.

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

## Argument type checks are missing on a broad set of library functions

Sampled, reproduced; the reference throws and phplang continues:

| call | reference | phplang |
|---|---|---|
| `new ArrayObject(1)` | `TypeError: ArrayObject::__construct(): Argument #1 ($array) must be of type array, int given` | the same `TypeError` naming the declared `object\|array` (the reference's constructor runs its own narrower check) |

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
| `try { goto out; } finally { … } out:` — a `goto` out of a `try`/`catch`/`finally` body | runs the `finally`, then jumps | a phplang compile error: a `try` body is a chunk of its own and the jump cannot leave it |
| `iconv_strlen("héllo")` | `int(5)` | `Call to undefined function iconv_strlen()` |
| `class A { public string $n { get => $this->n; } }` — PHP 8.4 property hooks | runs | a parse error: hooks have no lowering. The `RoundingMode` enum is likewise absent. Asymmetric visibility, the `|>` pipe, `clone($o, [...])` and the `(void)` cast are implemented |
| `clone($o, ...$args)`, `$f = fn(&$x) => 1; $g = f(...);` | argument unpacking in `clone()` evaluates; a first-class callable of a function with a by-reference parameter keeps the by-reference signature | a compile-time fatal for the unpacking; the callable the `f(...)` form builds is variadic by value, so `$g($v)` does not write `$v` back |
| `$o = (new A)->m(...)` where `A` has only `__call` — the callable's frame | the trace shows a frame `A->foo(1, 2)` with `__call` called from `[internal function]` | the trampoline is transparent, so `__call` is reported as called from the call site |
| `sort($GLOBALS['a'])`, `array_push($GLOBALS['a'], 1)` — a by-reference library argument rooted at `$GLOBALS[…]` | writes the global | the element is read through a copy, so the global is not changed |
| `$g = function (&$x) {}; $g(5);` — a non-variable argument for a by-reference parameter of a closure or callable string | `Error: …could not be passed by reference` | the call runs; the check is made for named functions and methods whose declaration is visible to the compiler |
| `var_dump(gmp_add(1, 2))`, `$a + $b` over GMP values, `gmp_setbit`, `gmp_clrbit`, `gmp_random_*`, `gmp_import`/`gmp_export` | `object(GMP)#1 (1) { ["num"]=> string(1) "3" }`; operator overloading | `string(1) "3"` — GMP values are decimal strings, so there is no object to dump, no operator overload (`+` over two big strings loses precision), and the by-reference and random functions that need one are absent. `GMP_VERSION` and the `GMP_MSW_FIRST` family are unseeded |
| `hash("ripemd160", "")`, `whirlpool`, `tiger*`, `snefru*`, `gost*`, `haval*`, `murmur3*`, `xxh*`, `md2`, `hash_init`/`hash_update`/`hash_final` | the digest | `ValueError`/undefined function: only `md4`, `md5`, `sha1`, `sha2*`, `sha3-*`, `adler32`, `crc32*`, `fnv*` and `joaat` are implemented, and `hash_algos()` lists exactly those |
| `ctype_alpha(chr(233))`, `ctype_alnum(255)` | follows the platform libc's locale tables (true on macOS, false on glibc) | always false for a byte at or above 0x80 — the glibc "C" locale |
| `assert_options(ASSERT_ACTIVE)`, `mt_srand(5, MT_RAND_PHP)` | the deprecated constants resolve (with a `Deprecated:` each) | `Undefined constant`: `ASSERT_*` and `MT_RAND_PHP` are not seeded and the legacy Mt19937 variant is not modelled |
| `f(... 1)` where `f` does not exist | `Error: Call to undefined function f()` | `TypeError: Only arrays and Traversables can be unpacked` — the unpack is checked before the callee is resolved |
| `Closure`/generator/enum-case handle numbers | `#N` counts them | see the object-handle entry below |
| `strtotime("2024-03-01 10:00 Europe/Paris")`, `new DateTimeZone("Europe/Paris")` — any zone identifier whose offset is not fixed | `int(1709283600)`; a zone | `false`; `DateInvalidTimeZoneException`: there is no tz database, so only `UTC` and its aliases and the `Etc/GMT±N` zones resolve (abbreviations such as `CEST` do) |
| `new DatePeriod("R2/2024-01-01T00:00:00Z/P1D")` — the deprecated ISO-string form | a period, with a deprecation | `TypeError`: only the date/interval forms are implemented |
| `$s = new SplStack; $s->push(1); var_dump((array) $s); var_export($s);` — likewise `SplHeap`, `SplPriorityQueue`, `SplFixedArray` | `array(0) {}`; `\SplStack::__set_state(array())` | the private properties the PHP-written prelude keeps the elements in (`flags`/`dllist`, `heap`, `__elements`). `var_dump`, `print_r`, `json_encode` and `serialize` match, through `__debugInfo` / `jsonSerialize` / `__serialize` |
| `class D { function __debugInfo() { return 5; } } var_dump(new D);` — a `__debugInfo` answer that is neither an array nor null, or one that throws | `Fatal error: __debuginfo() must return an array` | the object dumps with no properties (a throw propagates as a catchable exception); a nested `null` answer's deprecation is printed before the dump rather than inside it |
| `$f = new SplFixedArray(1); $f[] = 2;` | `Error: [] operator not supported for SplFixedArray` | `TypeError: Cannot access offset of type null on SplFixedArray` — the prelude's `offsetSet` cannot tell `$f[]` from `$f[null]`; an exception from a prelude `ArrayAccess` subscript also shows the `offsetGet`/`offsetSet` frame the reference's object handler does not |
| `foreach (new LimitIterator(new ArrayIterator([1])) as &$v) {}` — a by-reference `foreach` over an `Iterator` object | `Error: An iterator cannot be used with foreach by reference`; over an `ArrayIterator` it writes through to the storage | the iterator is walked into an array first and `$v` binds that copy: no error, and an `ArrayIterator`'s storage is not written. The by-value `foreach` is lazy and matches |
| `$it = new LimitIterator($inner); $it->count();` — a method forwarded to the inner iterator, as `spl_dual_it_get_method` forwards it | the inner method runs with no frame of its own for the forwarding | the same result, but the dual iterators declare a `__call` to do it, so `method_exists($it, "__call")` is true and `is_callable([$it, "anything"])` answers true |
| `$s = new SplObjectStorage; $s->attach(new stdClass);` — likewise `detach()`, `contains()` | `Deprecated: Method SplObjectStorage::attach() is deprecated since 8.5, use method SplObjectStorage::offsetSet() instead` | no deprecation. `#[\Deprecated]` is implemented, but `tests/spl.rs` (`spl_object_storage_by_identity`) pins these three calls' output without it, so the attribute is withheld until that expectation is revisited |
| `class C { function Alpha() {} } print_r(get_class_methods("C"));` | `Alpha` | `alpha`: the names come back lowercased. The order and the visibility filter are the reference's, but `tests/stdlib_reflection.rs` (`get_class_methods_lists_names_lowercased`) pins the lowercased spelling |

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
