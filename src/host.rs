//! The PHP object heap and runtime, reached from fusevm through registered
//! builtins (`builtins::install`) and the strict numeric hook.
//!
//! Scalars (int/float/bool/string/null) ride through the VM as native
//! `fusevm::Value`s. Arrays are heap objects: a `Value::Obj(u32)` handle indexes
//! `PhpHost::objs`. All mutable runtime state — the current variable scope stack,
//! the user-function table, the output buffer, the pending error/return signal —
//! lives in a `thread_local!` `PhpHost`, so a fresh `VM` can be spun up per
//! function call (see `call_function`) while sharing one heap.

use crate::ast::{TypeHint, Visibility};
use crate::errlevel;
use fusevm::{Chunk, VMResult, Value};
use indexmap::IndexMap;
use rustc_hash::{FxHashMap, FxHashSet};
use std::cell::{Cell, RefCell};

/// Builtin ids emitted by the compiler and registered on every VM.
pub mod ops {
    pub const ECHO: u16 = 1; // [parts...] argc=n -> Undef (prints each)
    pub const GETVAR: u16 = 2; // [name] -> value
    pub const SETVAR: u16 = 3; // [name, value] -> value
    pub const CONCAT: u16 = 4; // [a, b] -> string
    pub const TRUTHY: u16 = 5; // [v] -> Bool (PHP truthiness)
    pub const CALL: u16 = 6; // [name, args...] argc=1+n -> return value
    pub const MKARRAY: u16 = 7; // [k,v,...] argc=2n -> array handle
    pub const INDEX_GET: u16 = 8; // [recv, key] -> value
    pub const INDEX_SET: u16 = 9; // [name, key, val] -> val ($var[key] = val)
    pub const ARR_APPEND: u16 = 10; // [name, val] -> val ($var[] = val)
    pub const DIV: u16 = 11; // [a, b] -> a / b
    pub const MOD: u16 = 12; // [a, b] -> a % b
    pub const POW: u16 = 13; // [a, b] -> a ** b
    pub const LOOSE_EQ: u16 = 14; // [a, b] -> Bool (==)
    pub const LOOSE_NE: u16 = 15; // [a, b] -> Bool (!=)
    pub const STRICT_EQ: u16 = 16; // [a, b] -> Bool (===)
    pub const STRICT_NE: u16 = 17; // [a, b] -> Bool (!==)
    pub const LT: u16 = 18; // [a, b] -> Bool (<)
    pub const GT: u16 = 19; // [a, b] -> Bool (>)
    pub const LE: u16 = 20; // [a, b] -> Bool (<=)
    pub const GE: u16 = 21; // [a, b] -> Bool (>=)
    pub const SIG_RETURN: u16 = 22; // [v] -> halt function, return v
    pub const INCDEC: u16 = 23; // [name, code] -> value (++/--)
    pub const ARRAYKEYS: u16 = 24; // [arr] -> array of keys
    pub const ARRAYLEN: u16 = 25; // [arr] -> Int element count
    pub const BITAND: u16 = 26; // [a, b] -> a & b
    pub const BITOR: u16 = 27; // [a, b] -> a | b
    pub const BITXOR: u16 = 28; // [a, b] -> a ^ b
    pub const SHL: u16 = 29; // [a, b] -> a << b
    pub const SHR: u16 = 30; // [a, b] -> a >> b
    pub const BITNOT: u16 = 31; // [a] -> ~a
    pub const SPACESHIP: u16 = 32; // [a, b] -> -1 | 0 | 1
    pub const DBG_LINE: u16 = 33; // [line] -> DAP statement marker (debug only)
    pub const ARR_MUT: u16 = 34; // [name, subop, args...] -> by-reference array mutator
    pub const GET_PATH: u16 = 35; // [name, k1..kN] -> value (read $a[k1]..[kN])
    pub const SET_PATH: u16 = 36; // [name, k1..kN, val] -> val ($a[k1]..[kN] = val, N>=1)
    pub const APPEND_PATH: u16 = 37; // [name, k1..kM, val] -> val ($a[k1]..[kM][] = val)
    pub const INCDEC_PATH: u16 = 38; // [name, k1..kN, code] -> value (++/-- on element)
    pub const CALL_SPREAD: u16 = 39; // [name, flag, val, ...] -> return value (arg unpacking)
    pub const MKCLOSURE: u16 = 40; // [def_name, k, v, ...] argc=1+2n -> closure handle
    pub const CALL_VALUE: u16 = 41; // [callee, args...] argc=1+n -> return value
    pub const NEW: u16 = 42; // [class, args...] argc=1+n -> object handle
    pub const PROP_GET: u16 = 43; // [recv, name] -> value ($o->p)
    pub const PROP_SET: u16 = 44; // [recv, name, val] -> val ($o->p = v)
    pub const MCALL: u16 = 45; // [recv, method, args...] argc=2+n -> return
    pub const SCALL: u16 = 46; // [class, method, args...] argc=2+n -> return
    pub const SCONST: u16 = 47; // [class, name] -> class constant value
    pub const PATH_APPEND_CHILD: u16 = 48; // [name, k1..kN] -> handle of a freshly appended child array
    pub const PROP_ENSURE_ARRAY: u16 = 49; // [recv, name] -> handle of the array held in $o->name (vivified)
    pub const PROP_INCDEC: u16 = 50; // [recv, name, code] -> value (++/-- on $o->p)
    pub const THROW: u16 = 51; // [exc] -> record pending exception, halt chunk
    pub const RUN_TRY: u16 = 52; // [try-def id] -> control status int (see run_try)
    pub const SIG_HALT: u16 = 53; // [] -> halt chunk (propagate an already-set signal)
    pub const SIG_BREAK: u16 = 54; // [] -> signal `break`, halt chunk
    pub const SIG_CONTINUE: u16 = 55; // [] -> signal `continue`, halt chunk
    pub const CONST_FETCH: u16 = 56; // [name] -> constant value (or the bare name)
    pub const UNSET_VAR: u16 = 57; // [name] -> Undef (remove the scope variable)
    pub const UNSET_PATH: u16 = 58; // [name, k1..kN] -> Undef (remove $a[k1]..[kN])
    pub const FOREACH_PREP: u16 = 59; // [v] -> an iterable array (object -> iterated)
    pub const INSTANCEOF: u16 = 60; // [obj, class-name] -> Bool
    pub const REF_BIND: u16 = 61; // [target, source] -> value ($t = &$s)
    pub const BYREF_OUT: u16 = 62; // [position] -> the last call's by-ref param value
    /// `[name, value]` — a source-level `$x = expr`. Same as [`SETVAR`] but the
    /// copied, everything else passes through. Emitted by the compiler on the
    /// right-hand side of a source-level assignment only, so a compiler
    /// temporary and a `&` binding still share the handle they are given.
    /// See `PhpHost::copy_on_assign`.
    pub const COPY: u16 = 81;
    /// `[name]` — the enclosing variable's reference cell as a `PhpObj::Ref`
    /// handle, promoting the variable to a cell if it was not one. What
    /// `use (&$v)` captures; binding one into a frame aliases the name to the
    /// cell rather than storing the handle.
    pub const REF_CELL: u16 = 82;
    pub const SPROP_GET: u16 = 63; // [class, name] -> static property value (Class::$p)
    pub const SPROP_SET: u16 = 64; // [class, name, val] -> val (Class::$p = v)
    pub const SPROP_INCDEC: u16 = 65; // [class, name, code] -> value (++/-- on Class::$p)
    pub const STATIC_BIND: u16 = 66; // [name, slot-key, init] -> Undef (bind `static $var`)
                                     // Named-argument call variants (PHP 8.0). Each argument is encoded as a
                                     // `(name, value)` pair on the stack — `name` is a `Str` for a named argument
                                     // or `Undef` for a positional one — so the host can rebind by parameter name.
    pub const CALL_NAMED: u16 = 67; // [fname, (n,v)...] argc=1+2k -> return value
    pub const MCALL_NAMED: u16 = 68; // [recv, method, (n,v)...] argc=2+2k -> return
    pub const SCALL_NAMED: u16 = 69; // [class, method, (n,v)...] argc=2+2k -> return
    pub const NEW_NAMED: u16 = 70; // [class, (n,v)...] argc=1+2k -> object handle
    pub const CALLVALUE_NAMED: u16 = 71; // [callee, (n,v)...] argc=1+2k -> return value
                                         // Generators (host-side stackful coroutines). YIELD/YIELD_KV/YIELD_FROM run
                                         // inside a generator body and suspend it; the GEN_* ops drive a Generator from
                                         // a `foreach` (lazily, preserving side-effect order).
    pub const YIELD: u16 = 72; // [value] -> the value the next send()/next() supplies
    pub const YIELD_KV: u16 = 73; // [value, key] -> same, with an explicit key
    pub const YIELD_FROM: u16 = 74; // [iterable] -> delegate; leaves the delegate's return
    pub const IS_GENERATOR: u16 = 75; // [v] -> Bool (v is a Generator handle)
    pub const GEN_REWIND: u16 = 76; // [gen] -> Undef (prime to the first yield)
    pub const GEN_VALID: u16 = 77; // [gen] -> Bool (not yet finished)
    pub const GEN_KEY: u16 = 78; // [gen] -> current key
    pub const GEN_CURRENT: u16 = 79; // [gen] -> current value
    pub const GEN_NEXT: u16 = 80; // [gen] -> Undef (resume to the next yield)

    pub const SIG_LEVEL: u16 = 83; // [] -> Int, level of the last break/continue

    // `&` bindings to a *container slot* (`$r = &$a['x']['y']`, `$a[] = &$x`,
    // `$o->p = &$x`). Split into an ACQUIRE half, which returns the reference
    // cell the right-hand side denotes — promoting the element/property into a
    // reference if it was a plain value — and a BIND half, which points the
    // left-hand side at that cell. Composing the two halves covers every
    // combination of variable / element / property on either side without an
    // opcode per pair. The slot travels between them as a plain `Int`; it is a
    // compiler-internal value that never reaches a PHP expression.
    pub const REF_SLOT_VAR: u16 = 84; // [name] -> Int slot of $name
    pub const REF_SLOT_ELEM: u16 = 85; // [name, k1..kN] -> Int slot of $name[k1]..[kN]
    pub const REF_SLOT_PROP: u16 = 86; // [recv, prop] -> Int slot of $recv->prop
    pub const REF_TO_VAR: u16 = 87; // [name, slot] -> Undef ($name = &<slot>)
    pub const REF_TO_ELEM: u16 = 88; // [name, k1..kN, slot] -> Undef
    pub const REF_TO_APPEND: u16 = 89; // [name, k1..kM, slot] -> Undef ($name[..][] = &…)
    pub const REF_TO_PROP: u16 = 90; // [recv, prop, slot] -> Undef
    /// `[slot]` — a `function &f()` body's `return <lvalue>`: publish the slot as
    /// the call's returned reference and leave its value, which is what a plain
    /// (non-`&`) call of the function sees.
    pub const RET_REF: u16 = 91;
    /// `[]` — the reference cell the last call returned, for `$r = &f()`. A call
    /// that did not return by reference yields a fresh detached cell holding the
    /// returned value, so the binding degrades to a private copy rather than
    /// aliasing something arbitrary.
    pub const REF_SLOT_RET: u16 = 92;

    // Diagnostic-free reads. PHP fetches a variable / element / property in
    // "isset mode" (`BP_VAR_IS`) inside `isset()`, `empty()` and the left operand
    // of `??`, where a missing one is the question being asked rather than a
    // mistake, and raises no diagnostic. These are the same reads as their loud
    // twins with the warning suppressed; the compiler picks between them
    // statically, so a function call nested in a key still warns normally.
    pub const GETVAR_Q: u16 = 93; // [name] -> value, no "Undefined variable"
    pub const INDEX_GET_Q: u16 = 94; // [recv, key] -> value, no "Undefined array key"
    pub const PROP_GET_Q: u16 = 95; // [recv, name] -> value, no "Undefined property"

    /// `[recv, key] -> value` — one element of a `list()`/`[…]` destructuring.
    ///
    /// Separate from [`INDEX_GET`] because PHP treats a non-array subject
    /// differently here: `null` yields null silently, a scalar warns
    /// `Cannot use <type> as array`, and an object THROWS. Only an array
    /// reaches the ordinary keyed read.
    pub const LIST_ELEM_GET: u16 = 105;

    // Late static binding. `static::` names the class the call was made *on*, not
    // the class the running method was declared in, so it can only be resolved at
    // run time — a method inherited by two subclasses sees a different
    // `static::` in each.
    /// `[fallback]` -> the running frame's late-static-binding class. The
    /// compiler supplies the enclosing class as the fallback, which is what the
    /// name means outside any call (and what `self::` always means).
    pub const LSB_CLASS: u16 = 96;
    /// `[]` -> Undef. Marks the next call as *forwarding*: `self::m()`,
    /// `parent::m()` and `static::m()` keep the caller's late-static-binding
    /// class, whereas naming a class explicitly (`Base::m()`) replaces it.
    pub const LSB_FORWARD: u16 = 97;

    /// `[recv, name]` -> `recv`, unchanged. Marks a property as being fetched
    /// FOR WRITING, which is when PHP decides whether the write creates a dynamic
    /// property. Emitted ahead of the read half of a compound assignment
    /// (`$o->p .= "x"`), because the reference engine raises the
    /// "Creation of dynamic property" deprecation before the "Undefined property"
    /// warning the read then produces — the slot exists by the time it is read.
    pub const PROP_TOUCH: u16 = 98;
    /// `[recv, name, val]` -> `val`. The write half of a read-modify-write, whose
    /// creation of a dynamic property was already announced by the [`PROP_TOUCH`]
    /// that opened it. Identical to [`PROP_SET`] but silent, so `$o->p .= "x"`
    /// deprecates once rather than twice.
    pub const PROP_SET_RW: u16 = 99;
    /// `[] -> null`. Open an `@expr` suppression region (see
    /// [`super::PhpHost::suppress_push`]). The pushed null is dropped by the
    /// `Pop` the compiler emits after it.
    ///
    /// `@` needs a RUN-TIME region, not just the quiet read opcodes: a warning
    /// raised inside a library function (`@preg_match('/[a', $s)`,
    /// `@range('ab', 'c')`) is raised from Rust with no opcode of its own to
    /// quieten, and the reference silences those too.
    pub const SUPPRESS_PUSH: u16 = 100;
    /// `[value] -> value`. Close the region [`SUPPRESS_PUSH`] opened, passing the
    /// operand's value through so `@` stays an expression.
    pub const SUPPRESS_POP: u16 = 101;
    /// `[recv, name] -> null`. `unset($o->p)`.
    pub const PROP_UNSET: u16 = 102;
    /// `[recv, name] -> bool`. `isset($o->p)` — see [`super::PropAccess`] and the
    /// `IssetOf` arm of the compiler for why this cannot be a value comparison.
    pub const PROP_ISSET: u16 = 103;
    /// `[recv, name] -> value`. The read inside `empty($o->p)`. [`PROP_GET_Q`]
    /// with one arm changed: a property no `__isset` vouched for reads as null
    /// instead of falling back to `__get`, so a class with `__get` and no
    /// `__isset` is `empty()` without `__get` being called at all.
    pub const PROP_GET_EMPTY: u16 = 104;
    /// `[recv, key] -> bool`. `isset($a[k])`. An array or string answers from its
    /// own contents, but an `ArrayAccess` object answers with `offsetExists` and
    /// NOTHING else — `offsetGet` is never called, so the answer cannot be
    /// recovered from a value the way it can for an array.
    pub const INDEX_ISSET: u16 = 106;

    /// `[message] -> !`. A declaration the compiler read but the reference would
    /// refuse to LINK — today, a `use Trait` whose adaptations do not resolve.
    ///
    /// It is an op rather than a compile error because PHP binds a
    /// trait-using class at run time: everything the script printed before the
    /// declaration is printed, and only then does the fatal land. Emitting it
    /// where the declaration stood reproduces that ordering exactly.
    pub const DECL_FATAL: u16 = 107;

    /// `[value] -> class-name`. The left of a `::` when it is an expression
    /// rather than a bareword (`$cls::K`, `$obj::m()`): an object resolves to
    /// its class, a string is already a class name, and anything else is an
    /// `Error` — PHP does not coerce here.
    pub const DYN_CLASS: u16 = 108;

    /// `[value] -> class-name`. `$expr::class`, which is NOT `DYN_CLASS`
    /// followed by a read: PHP answers only for an object and rejects even the
    /// class-name string that `DYN_CLASS` would happily accept.
    pub const DYN_CLASS_CONST: u16 = 109;

    /// `[object] -> object`. `clone $o`: a new instance of the same class with
    /// the properties copied (arrays by value, handles shared), then `__clone`
    /// if the class defines one.
    pub const CLONE: u16 = 110;

    /// `[name, value] -> Undef`. The `const NAME = expr;` declaration.
    ///
    /// An op rather than a compile-time table write because the declaration
    /// takes effect WHERE IT STANDS: a `defined()` earlier in the script must
    /// answer false, and a redefinition must warn at the point of the second
    /// declaration. Both need the statement to run in source order.
    pub const CONST_DECL: u16 = 111;

    /// `[prefix, suffix] -> string`. `{prefix}{__FILE__}{suffix}` — the running
    /// script's name, optionally wrapped. The affixes are how a closure declared
    /// at file scope gets its `{closure:<file>:<line>}` name.
    pub const MAGIC_FILE: u16 = 112;

    /// `[] -> string`. `__DIR__`.
    pub const MAGIC_DIR: u16 = 113;

    /// `[prefix, suffix] -> string`. `{prefix}{__CLASS__}{suffix}` where the parse
    /// could not name the class — inside a trait method (the USING class) or an
    /// anonymous class. The affixes build such a class's `__METHOD__`.
    pub const MAGIC_CLASS: u16 = 114;

    /// `[pos] -> Bool`. Whether the call that just returned took its parameter at
    /// `pos` by reference. Guards the write-back at a call site whose callee is
    /// not a compile-time fact — a method call, a static call, or `$f(…)`.
    pub const BYREF_LIVE: u16 = 115;

    // ── slot-addressed locals ────────────────────────────────────────────────
    // The compiler resolves a scope's variables to indices and emits these
    // instead of the by-name ops above, so a read is a vector index rather than
    // a superglobal-name test plus a string hash into the scope map. The slot
    // and the name address the SAME storage (`Vars`), so `extract()`, `$$name`,
    // `unset()`, a reference binding, an array path write and a by-reference
    // parameter's write-back all stay coherent with slot access — they simply
    // reach the slot the other way round.
    /// `[] -> value` — read slot N, warning `Undefined variable` if unbound.
    pub const GETSLOT: u16 = 116;
    /// `[] -> value` — read slot N with no diagnostic (isset/`empty` context).
    pub const GETSLOT_Q: u16 = 117;
    /// `[value] -> value` — write slot N, through the shared cell if it is a
    /// reference binding.
    pub const SETSLOT: u16 = 118;
    /// `[slot, code] -> value` — `++`/`--` on slot N. One op for the whole
    /// read-modify-write: the by-name form resolved the name twice per
    /// execution, once to read and once to store, which in a counted loop is
    /// two hashes an iteration for a variable the compiler had already placed.
    pub const INCDEC_SLOT: u16 = 119;
    /// `[arr, k,v, ...] argc=1+2n -> arr` — append more `key => value` pairs to
    /// an array already built by `MKARRAY`.
    ///
    /// `CallBuiltin`'s operand count is a `u8`, so one `MKARRAY` can carry at
    /// most 127 pairs. A longer literal is emitted as an `MKARRAY` followed by
    /// `MKARRAY_ADD` continuations rather than overflowing that count — which
    /// is what a 200-element literal used to do (`400 as u8` == 144, so the
    /// builtin popped 144 of its 400 operands and left the rest on the stack).
    pub const MKARRAY_ADD: u16 = 120;
    /// `[name, lhs, rhs] -> value` — a DIRECT two-argument `min()`/`max()`.
    ///
    /// The reference compiles that exact call shape to a different C function
    /// than every other way of reaching `min`, and the two do not agree once a
    /// NaN is involved (see `minmax_frameless`). Selecting
    /// between them is a compile-time fact there, so it is one here too: the
    /// compiler emits this op only for a literal two-argument call, and
    /// everything else — a dynamic name, `call_user_func`, a spread, a named
    /// argument, any other arity — keeps [`CALL`] and the ordinary function.
    ///
    /// The name still travels with it so a user-declared `min`/`max` is found
    /// first, exactly as [`CALL`] would.
    pub const MINMAX_FLF2: u16 = 121;
    /// `[kind, callee, argno, param] -> null` — the by-reference ARGUMENT check.
    ///
    /// A by-reference parameter needs somewhere to write back to, and not every
    /// expression can supply one. The reference sorts them into three groups and
    /// treats each differently, so the compiler classifies the argument and emits
    /// this op to carry the verdict into the run:
    ///
    /// * a variable, a subscript, a property or a static property is a real
    ///   location and no diagnostic is emitted at all — this op is not emitted;
    /// * a function/method call or a `new` produces a fresh temporary the engine
    ///   CAN bind, so `kind` 0 raises `Notice: Only variables should be passed by
    ///   reference` and the call proceeds against that temporary;
    /// * anything else — a literal, a ternary, an assignment, a cast, `clone`,
    ///   `@expr`, `?->`, `??` — has no location even in principle, so `kind` 1
    ///   throws `Error: {callee}(): Argument #{argno} (${param}) could not be
    ///   passed by reference` and the call never runs.
    ///
    /// `param` is empty for a VARIADIC by-reference position (`sscanf`), whose
    /// message names no parameter.
    ///
    /// The op sits between the argument it judges and the rest of the argument
    /// list, because that is where the reference performs the check: the failing
    /// argument is evaluated for its side effects first, and the arguments AFTER
    /// it are never evaluated at all.
    pub const BYREF_ARG_DIAG: u16 = 122;
    /// `[name] -> null` — `global $name;`.
    ///
    /// Binds the running frame's `name` as an ALIAS of the global variable of
    /// that name, which is what the reference does: `global $x` is defined as
    /// `$x = &$GLOBALS['x']`, so the two share one cell and each sees the
    /// other's writes. One op per name keeps `global $a, $b;` a plain sequence.
    pub const GLOBAL_BIND: u16 = 123;
    /// `[old, inc] -> new` — one step of `++`/`--` on a VALUE.
    ///
    /// The slot and by-name forms of `++` read the variable, step it and write
    /// it back in one builtin, because they are the only ones that know where
    /// the variable lives. A local promoted to a fusevm frame slot is read and
    /// written by the ops themselves, so all it needs from the host is the step
    /// — which is not arithmetic: `"Az"++` is `"Ba"` and `null--` is a no-op.
    pub const INCDEC_STEP: u16 = 124;
    /// `[subject] -> string` — the `UnhandledMatchError` message for a `match`
    /// that matched no arm.
    ///
    /// Not a plain concatenation of the subject: the reference renders the
    /// value the way a stack trace renders an argument (a string single-quoted,
    /// escaped and cut at 15 bytes; a float keeping its `.0`; `true`/`false`/
    /// `NULL` as words) and replaces anything that is not a scalar with `of
    /// type <type>`. Concatenating instead turned `null` into the empty string,
    /// `true` into `1`, `'hi'` into unquoted `hi`, and an array into `Array`
    /// behind an `Array to string conversion` warning the reference never
    /// raises.
    pub const MATCH_ERROR_MSG: u16 = 125;
    /// `[] -> string` — the class `self::` names inside a TRAIT method.
    ///
    /// A trait's methods are compiled once and copied into every class that
    /// uses them, so `self` cannot be baked while the body is lowered: it is
    /// the composing class, which is only known once a frame is running. Baking
    /// it made `self::class` answer the TRAIT's name from every class that used
    /// it, and `new self()` build an instance of the trait.
    pub const SELF_CLASS: u16 = 126;
    /// `[] -> string` — the class `parent::` names inside a TRAIT method: the
    /// parent of the composing class, for the same reason as
    /// [`SELF_CLASS`]. Baking it refused every trait that spelled
    /// `parent::` with `'parent' used in a class with no parent`, whatever the
    /// using class extended.
    pub const PARENT_CLASS: u16 = 127;
    /// `[recv, method]` -> `recv`. Raises `Call to a member function m() on
    /// <type>` when `recv` is not an object, WITHOUT touching the method table.
    /// Emitted before a method call's arguments, because the reference decides
    /// a receiver is uncallable before it evaluates any of them.
    pub const MCALL_RECV_CHECK: u16 = 128;
    /// `[recv, key] -> value` — [`INDEX_GET_Q`] with `empty()`'s wording for a
    /// bad offset TYPE. Everything else about the read is identical; the two
    /// contexts differ only in the sentence the reference ends the message with.
    pub const INDEX_GET_EMPTY: u16 = 129;
    /// `[name]` -> `name`. Raises `Call to undefined function f()` when no
    /// function of that name resolves. Emitted before a named call's arguments
    /// for the same reason as [`MCALL_RECV_CHECK`]: the reference decides the
    /// callee before it evaluates a single argument, so `undefined(f())` must
    /// not run `f`.
    pub const CALL_NAME_CHECK: u16 = 130;
    /// `[class]` -> `class`. Raises `Class "X" not found` for an undeclared
    /// class. Emitted before the arguments of `new X(...)` and `X::m(...)`,
    /// which both reject an unknown class before evaluating them.
    pub const CALL_CLASS_CHECK: u16 = 131;
    /// `[callee]` -> `callee`. Raises whatever `$callee(...)` was going to raise
    /// for a value that is not callable at all, before the arguments run.
    pub const CALLVALUE_CHECK: u16 = 132;
    /// `[class, method]` -> `class`. The whole of `C::m(…)`'s callee decision —
    /// the class exists, the method is declared, and it is reachable from here —
    /// run before the arguments. Subsumes [`CALL_CLASS_CHECK`] for the static
    /// call form, which is why that op is not also emitted there.
    pub const SCALL_CALLEE_CHECK: u16 = 133;
    /// `[callable, is_instance_form]` -> `callable`. The callee decision of
    /// PHP 8.1 first-class callable syntax, `callee(...)`, made where the syntax
    /// is WRITTEN. The reference refuses an undefined function, an undeclared
    /// class, an undefined or unreachable method and a non-object receiver at
    /// that point — the closure is never built — so none of it can wait for the
    /// call the closure may never receive.
    pub const FCC_CHECK: u16 = 134;
    /// `[class, name]` -> value. `Class::$p` read in `BP_VAR_IS` mode, for
    /// `isset`/`empty`/`??`: an undeclared or unreachable static property reads
    /// as null instead of throwing (`zend_fetch_static_property_address` with
    /// `BP_VAR_IS`). An unknown CLASS still throws, as it does in the reference.
    pub const SPROP_GET_Q: u16 = 135;
    /// `[path, kind]` -> value. `include`/`require` and their `_once` forms:
    /// load, compile and run a file in the current frame (`host::run_include`).
    pub const INCLUDE: u16 = 136;
    /// `[code]` -> value. `eval()`: compile and run a string in the current frame.
    pub const EVAL: u16 = 137;
    /// `[key, name, namespace]` -> null. A function declared anywhere but a
    /// file's top level (inside an `if`, a loop or another function's body):
    /// the reference declares it when the statement RUNS, so until then the
    /// name is undefined, and a second run is a redeclaration. The compiled
    /// definition waits in the function table under `key`, which no PHP name
    /// can spell, until this moves it to its real name.
    pub const DECLARE_FN: u16 = 138;
    /// `[key, name, namespace]` -> null. [`DECLARE_FN`] for a class, interface,
    /// trait or enum declared inside a block.
    pub const DECLARE_CLASS: u16 = 139;
    /// `[class]` -> null. The `final` rules a class declared where it stands
    /// must keep against what it inherits (see [`final_violation`](super::final_violation)), checked
    /// when the declaration RUNS: until then its parent may not be declared.
    /// A broken rule stops the program like [`DECL_FATAL`].
    pub const FINAL_CHECK: u16 = 140;
    /// `[value, mark] -> null`. A `foreach` released its subject (the loop
    /// ended, or a `return` left it). A suspended generator created while the
    /// subject was evaluated (its id is at or past `mark`, see [`GEN_MARK`])
    /// that no variable, element or property holds is destroyed: the `finally`
    /// blocks around the `yield` it is parked at run now, as they do when the
    /// reference frees it.
    pub const GEN_RELEASE: u16 = 141;
    /// `[] -> int`. The id the next generator will get. Taken before a
    /// `foreach` subject is evaluated, for [`GEN_RELEASE`].
    pub const GEN_MARK: u16 = 142;
    /// `[recv, name] -> value`. `$recv->name` fetched FOR WRITING, as the link
    /// of a longer write target (`$recv->name->p = v`). A missing property is
    /// created as null with no `Undefined property` warning — the write after
    /// it is what fails — and a receiver that cannot hold one is an `Error`
    /// (`Attempt to modify property "name" on null`).
    pub const PROP_FETCH_W: u16 = 143;
    /// `[recv-or-class, method, argno, kind] -> null`. [`BYREF_ARG_DIAG`] for a
    /// method or static call, whose callee is only known at run time: the
    /// method is resolved on the receiver (an object, or a class name for
    /// `C::m()`), and the diagnostic is raised only when its parameter at
    /// `argno` is by reference. `kind` is the compiler's verdict on the
    /// argument, as for `BYREF_ARG_DIAG`. A method that does not resolve —
    /// one `__call` answers — takes nothing by reference.
    pub const BYREF_ARG_DIAG_M: u16 = 144;
    /// `[class] -> object`. The first half of `new`: allocate the instance
    /// (property defaults included) BEFORE the constructor arguments are
    /// evaluated, as `ZEND_NEW` does — it decides the `#N` the object gets.
    pub const NEW_ALLOC: u16 = 145;
    /// `[object, args...]` argc=1+n `-> object`. The second half: run the
    /// constructor over the instance [`NEW_ALLOC`] made.
    pub const NEW_INIT: u16 = 146;
    /// `[object, (n,v)...]` argc=1+2k `-> object`. [`NEW_INIT`] with named
    /// arguments.
    pub const NEW_INIT_NAMED: u16 = 147;
    /// `[class, name] -> array`. `Class::$p` fetched to be written through
    /// (`Class::$p[] = v`, `Class::$p[k]++`): its array, vivified when the
    /// property holds none.
    pub const SPROP_ENSURE_ARRAY: u16 = 148;
    /// `[v] -> v`'s iterator. A `foreach` over an `IteratorAggregate` walks the
    /// iterator its `getIterator()` answers; anything else is its own subject.
    pub const FOREACH_ITER: u16 = 149;
    /// `[v] -> Bool`: whether a `foreach` walks `v` one step at a time — a
    /// generator, or an object implementing `Iterator` — rather than as an
    /// array of its elements.
    pub const IS_LAZY_ITER: u16 = 150;
}

/// The capture name a `static` closure carries from its creation site.
///
/// `$` cannot begin a PHP variable name, so this can never collide with one the
/// program wrote, and it travels through the ordinary capture list rather than
/// needing an operand of its own.
pub const STATIC_CLOSURE_CAPTURE: &str = "@static-closure";

/// The key operand of an array-literal element that has NO key — `[1, 2]`
/// rather than `[0 => 1, 1 => 2]`.
///
/// It cannot be `Value::Undef`: that is how PHP `null` rides through the VM, so
/// `[null => "a"]` and `["a"]` would compile to the same operand sequence.
/// They did, and the first one produced the integer key `0` instead of `""`.
/// `Value::Status` carries no PHP value at all, so no expression can forge it.
pub const AUTO_INDEX: Value = Value::Status(i32::MIN);

/// The key operand marking a `...` SPREAD element in an array literal.
///
/// A spread contributes a whole array's worth of entries rather than one, so it
/// cannot ride as a `key => value` pair. Like [`AUTO_INDEX`] it is a
/// `Value::Status`, which carries no PHP value, so no expression can forge it —
/// and it is a DIFFERENT status, so `[...$a]` and `[$a]` stay distinguishable.
pub const SPREAD_KEY: Value = Value::Status(i32::MIN + 1);

/// `array_filter()` `$mode`: the callback is handed the KEY alone.
pub const ARRAY_FILTER_USE_KEY: i64 = 2;
/// `array_filter()` `$mode`: the callback is handed the VALUE and the key.
pub const ARRAY_FILTER_USE_BOTH: i64 = 1;

/// Whether a key operand is the [`AUTO_INDEX`] marker rather than a real key.
pub fn is_auto_index(v: &Value) -> bool {
    matches!(v, Value::Status(n) if *n == i32::MIN)
}

/// Whether a key operand is the [`SPREAD_KEY`] marker.
pub fn is_spread_key(v: &Value) -> bool {
    matches!(v, Value::Status(n) if *n == i32::MIN + 1)
}

/// The largest number of `key => value` pairs one `MKARRAY`/`MKARRAY_ADD` can
/// carry, bounded by `CallBuiltin`'s `u8` operand count (`MKARRAY_ADD` spends
/// one of the 255 on the array itself).
pub const MKARRAY_CHUNK_PAIRS: usize = 127;

/// Sub-ops for the by-reference array mutators lowered through `ops::ARR_MUT`
/// (`array_push`/`array_pop`/`array_shift`/`array_unshift`/`array_splice`). These
/// take the array by variable name so the host can rewrite the bound array (and
/// auto-vivify an unset variable) the way PHP's by-reference parameter does.
pub mod arrmut {
    pub const PUSH: i64 = 0;
    pub const POP: i64 = 1;
    pub const SHIFT: i64 = 2;
    pub const UNSHIFT: i64 = 3;
    pub const SPLICE: i64 = 4;
}

/// A compiled formal parameter: its name, an optional default-value chunk (run in
/// the callee scope when the caller omits the argument), and whether it collects
/// all trailing arguments into an array (`...$rest`).
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    /// The line the parameter is DECLARED on — where PHP reports a coercion
    /// `Deprecated` against, rather than the call site.
    pub line: u32,
    pub default: Option<Chunk>,
    pub variadic: bool,
    /// `&$x` — a by-reference parameter (final value copied back to the caller).
    pub by_ref: bool,
    /// The declared type, carried from the source so the bind can check it.
    pub ty: Option<TypeHint>,
}

/// A compiled user function: its parameters plus the lowered body chunk.
#[derive(Debug, Clone)]
pub struct FuncDef {
    pub params: Vec<Param>,
    pub chunk: Chunk,
    /// The frame's variables in the order the chunk numbers them. The call
    /// reserves these slots before binding anything, so slot `n` in the body and
    /// slot `n` in the frame are the same variable. Empty when the body was
    /// compiled to address its variables by name.
    pub locals: Vec<String>,
    /// True when the body contains a `yield`: calling it builds a suspended
    /// `Generator` (a host-side stackful coroutine) instead of running the body.
    pub is_generator: bool,
    /// The declared return type, checked on the way out of the call.
    pub ret: Option<TypeHint>,
    /// Set only for a closure body: the site of the literal that declared it,
    /// which is what names its stack frames. `None` for a named function or
    /// method, whose frame is named by the function itself.
    pub closure_site: Option<DeclSite>,
    /// A named function's name as DECLARED, which is how its frames and its
    /// argument errors name it however a call spells it: `function Foo()`
    /// called as `FOO()` is `Foo()` in a trace. `None` for methods and closures.
    pub declared: Option<String>,
    /// Its `#[\Deprecated]` attribute: calling it raises the deprecation first.
    pub deprecated: Option<crate::ast::Deprecation>,
}

/// Where a closure literal was WRITTEN, which is what PHP 8.4 names a closure
/// frame by: `{closure:<where>:<line>}`.
///
/// `<where>` is the enclosing declaration — `K::m()` for a method, `outer()` for
/// a function, the script path at the top level — and it nests, so a closure
/// inside a closure reads `{closure:{closure:f.php:2}:3}`. The path is only
/// known once the script is running, so the top level stays a marker until then.
/// `Closure::bind` changes none of it: the site is the literal's, not the call's.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum DeclSite {
    /// Written at the top level of the script; PHP prints the file path. This is
    /// the default because it is where lowering starts.
    #[default]
    Script,
    /// Written inside a named function or method, spelled as PHP spells it in
    /// this position: `outer()`, `K::m()`.
    Named(String),
    /// Written inside another closure literal, itself declared at that site and
    /// on that line.
    Closure(Box<DeclSite>, u32),
}

impl DeclSite {
    /// The `<where>` half of a `{closure:<where>:<line>}` name, with `script`
    /// standing in for the top level.
    pub fn render(&self, script: &str) -> String {
        match self {
            DeclSite::Script => script.to_string(),
            DeclSite::Named(name) => name.clone(),
            DeclSite::Closure(site, line) => {
                format!("{{closure:{}:{line}}}", site.render(script))
            }
        }
    }
}

/// A closure unpacked for a call: its parameters, body chunk, captured bindings,
/// and any `Closure::bind` rebinding (`$this` object + private-access scope class).
pub struct ClosureCall {
    pub params: Vec<Param>,
    pub chunk: Chunk,
    pub captured: Vec<(String, Value)>,
    pub bound_this: Option<Value>,
    pub scope: Option<String>,
    /// The late-static-binding class (`zend_closure.called_scope`).
    pub called_scope: Option<String>,
    pub is_generator: bool,
    /// The declared return type, checked on the way out of the call.
    pub ret: Option<TypeHint>,
    /// The site of the closure literal, carried through so the frame it opens
    /// can be named the way PHP names it. See `DeclSite`.
    pub site: Option<DeclSite>,
}

/// A compiled class: its parent (for single-inheritance resolution), constant and
/// property-default initializers (each an expression chunk that leaves its value
/// on the stack), and its methods keyed by lowercase name. `self::`/`parent::`
/// were resolved to concrete class names at compile time.
#[derive(Debug, Clone)]
pub struct ClassDef {
    /// The class name in its declared spelling. The map is keyed by the
    /// lowercased name (PHP class lookup is case-insensitive), so this is the
    /// only place the source casing survives for display — a stack trace names
    /// the class that DEFINED the method, which may have no live instance to
    /// read the spelling back from.
    pub name: String,
    pub parent: Option<String>,
    /// Implemented interfaces (lowercased) — for an `interface`, the interfaces it
    /// extends. Consulted by `class_is_a`/`instanceof`/`catch`.
    pub interfaces: Vec<String>,
    pub consts: Vec<(String, Chunk)>,
    /// Declared visibility of the `private`/`protected` constants declared in
    /// THIS class (a trait's are copied in by the class that uses it). A name
    /// absent here is `public`.
    pub const_vis: FxHashMap<String, Visibility>,
    pub prop_defaults: Vec<(String, Chunk)>,
    /// `static` property declarations (`public static $n = 0;`). Kept apart from
    /// `prop_defaults` so they are never copied into an instance; each initializer
    /// runs once, on first access, into the host's per-class static store.
    pub static_prop_defaults: Vec<(String, Chunk)>,
    pub methods: FxHashMap<String, FuncDef>,
    /// Declared visibility of properties declared in THIS class (instance and
    /// static), by property name. Consulted (walking the parent chain) to enforce
    /// `private`/`protected` on external access.
    pub prop_vis: FxHashMap<String, Visibility>,
    /// Properties THIS class declares `readonly` (a promoted constructor
    /// parameter counts as a declaration). Looked up along the parent chain, so
    /// the class named in `Cannot modify readonly property C::$p` is the one
    /// that declared it, not the one the write went through.
    pub readonly_props: FxHashSet<String>,
    /// Typed instance properties THIS class declares with no default, by name,
    /// each with its type as `var_dump` spells it. They start UNINITIALIZED —
    /// absent from a new instance — and reading one before a write is an
    /// `Error`. A trait's are copied in by the class that uses it.
    pub uninit_props: FxHashMap<String, String>,
    /// Every typed property THIS class (or a trait it uses) declares, instance
    /// and static, by name — what a write to it is checked against.
    pub prop_types: FxHashMap<String, crate::ast::TypeHint>,
    /// Declared visibility of methods declared in THIS class), by lowercased name.
    pub method_vis: FxHashMap<String, Visibility>,
    /// Methods THIS class declares `static`, by lowercased name. Looked up along
    /// the parent chain by [`PhpHost::method_is_static`], which is what tells a
    /// `["C", "m"]` callable apart from a `[$obj, "m"]` one: the reference
    /// refuses `["C", "m"]` for a non-static `m` with
    /// `non-static method C::m() cannot be called statically`.
    pub static_methods: FxHashSet<String>,
    /// Whether this class is an `enum` (PHP 8.1).
    pub is_enum: bool,
    /// Whether the class is `abstract` or an `interface` — either way, `new` on it
    /// is a fatal error in PHP.
    pub is_abstract: bool,
    pub is_interface: bool,
    /// Whether this entry is a `trait`. Traits share the class table (their
    /// members are seeded into every composing class from it), but they are a
    /// different KIND to the reflection surface: `trait_exists` answers yes and
    /// `class_exists` answers no.
    pub is_trait: bool,
    /// Traits named by `use` in this declaration, in source order and declared
    /// spelling. Kept after the members have been merged, because `class_uses`
    /// reports which traits were composed and the merge itself erases that.
    pub uses: Vec<String>,
    /// Whether the class carries `#[AllowDynamicProperties]`, which opts it — and
    /// everything that extends it — out of the "Creation of dynamic property"
    /// deprecation. Checked by walking the parent chain, because the attribute is
    /// inherited.
    pub allow_dynamic_props: bool,
    /// `enum` cases, in source order: `(case name, optional backing-value chunk)`.
    /// Consulted to build the singleton case instances (`E::Case`, `E::cases()`,
    /// `E::from()`, `E::tryFrom()`).
    pub enum_cases: Vec<(String, Option<Chunk>)>,
    /// Declared `final`: no class may extend it.
    pub is_final: bool,
    /// Methods THIS class (or a trait it uses) declares `final`, by lowercased
    /// name. A `private` one is left out unless it is the constructor: the
    /// reference never checks an override of a private method, since a
    /// private method is not inherited.
    pub final_methods: FxHashSet<String>,
    /// Constants THIS class (or interface, or used trait) declares `final`.
    pub final_consts: FxHashSet<String>,
    /// Properties THIS class (or a used trait) declares `final`.
    pub final_props: FxHashSet<String>,
    /// Every method THIS class declares or takes from a trait, as
    /// `(declared spelling, line of its declaration)`, in declaration order.
    /// A broken `final` rule names the overriding method's spelling and is
    /// reported at its line, and the reference checks the parent's methods in
    /// the order the parent declared them.
    pub method_sites: Vec<(String, u32)>,
}

/// One compiled `catch (T1 | T2 [$var]) { body }` clause of a `try`.
#[derive(Debug, Clone)]
pub struct CatchClause {
    pub classes: Vec<String>,
    pub var: Option<String>,
    pub chunk: Chunk,
}

/// A compiled `try`/`catch`/`finally` construct: each body is its own detached
/// chunk, run by the `run_try` orchestrator on the *current* scope (no new frame),
/// so variables are shared with the enclosing code.
#[derive(Debug, Clone)]
pub struct TryDef {
    pub try_chunk: Chunk,
    pub catches: Vec<CatchClause>,
    pub finally_chunk: Option<Chunk>,
}

/// A PHP array key — always an integer or a string (bool/float/null keys are
/// normalized to one of these on insert, as PHP does).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ArrayKey {
    Int(i64),
    Str(String),
}

impl ArrayKey {
    /// The key as a PHP `Value` (for `array_keys`/`foreach`).
    fn to_value(&self) -> Value {
        match self {
            ArrayKey::Int(n) => Value::int(*n),
            ArrayKey::Str(s) => Value::str(s.clone()),
        }
    }
}

/// A heap object. Arrays and closures live here in the scaffold; user-defined
/// objects are a later wave.
#[derive(Debug, Clone)]
pub enum PhpObj {
    Array {
        entries: IndexMap<ArrayKey, Value>,
        /// The next integer key an append (`$a[] = ...`) will use.
        next_index: i64,
    },
    /// A first-class callable: its parameters, its lowered body chunk, and the
    /// `(name, value)` bindings captured (by value) at creation time. `bound_this`
    /// / `scope` are set by `Closure::bind`/`bindTo`/`call`: the bound `$this`
    /// object and the class name whose private/protected members the body may reach.
    Closure {
        params: Vec<Param>,
        /// Boxed to keep the `Closure` variant from dominating `PhpObj`'s size
        /// (a `Chunk` is ~300 bytes; boxing it shrinks the enum to a handle).
        chunk: Box<Chunk>,
        captured: Vec<(String, Value)>,
        bound_this: Option<Value>,
        scope: Option<String>,
        /// The late-static-binding class its frames run with — what `static::`
        /// and `get_called_class()` answer inside it (`zend_closure.called_scope`).
        /// Set where the closure is created or rebound, never at the call.
        called_scope: Option<String>,
        is_generator: bool,
        /// The declared return type, checked on the way out of a call.
        ret: Option<TypeHint>,
        /// Declared `static`, so no instance may ever be bound to it — at
        /// creation, and by `Closure::bind`/`bindTo` afterwards.
        is_static: bool,
        /// Where the literal was written, which names the frames it opens. It
        /// travels with the closure VALUE, so `Closure::bind` cannot move it.
        site: Option<DeclSite>,
    },
    /// A live generator: an index into `PhpHost::generators`, where the suspended
    /// stackful coroutine and its execution context live. Built by calling a
    /// generator function; iterated by `foreach` or the `Generator` methods.
    Generator { id: u32 },
    /// A class instance: its class name and its properties. Referenced by a
    /// `Value::Obj(u32)` handle, so objects have PHP reference semantics (passing
    /// one around shares the same instance).
    Object {
        class: String,
        props: IndexMap<String, Value>,
    },
    /// A handle to a variable's reference cell, which is what `use (&$v)`
    /// captures. It is never a value a PHP script can hold: it is created at
    /// closure creation and consumed when the closure's frame is built, where
    /// it binds the frame's name to the cell instead of storing the handle.
    Ref { slot: usize },
    /// A stream resource (`fopen`, `tmpfile`, `STDIN`, …). A `Value::Obj`
    /// handle, so every copy of the value shares one cursor. Boxed to keep the
    /// variant from dominating `PhpObj`'s size.
    Resource(Box<crate::stdlib::fileres::Stream>),
}

/// A control-flow signal that unwinds out of a function body. `Throw` rides a
/// separate `pending_throw` field (not this enum) so an in-flight exception is
/// never confused with a `return`/`break`/`continue` and survives frame unwinding.
enum Signal {
    Return(Value),
    /// `break n` — the payload is how many loop levels are still to be left,
    /// so a `try` that sits inside fewer loops than that can decrement it and
    /// re-raise for the next frame out.
    Break(u32),
    /// `continue n` — see [`Signal::Break`].
    Continue(u32),
}

/// What one name in a scope is bound to.
#[derive(Default, Clone)]
enum Slot {
    /// Never written, or `unset()`. Distinct from a slot holding PHP `null`
    /// (`Value::Undef`), and the difference is what `isset()` and `compact()`
    /// answer on: `$a = null; compact('a')` yields `['a' => null]`, an unset
    /// `$a` yields `[]`.
    #[default]
    Unset,
    Val(Value),
    /// Bound as a reference (`$b = &$a`): the value lives in a shared
    /// `PhpHost::ref_cells` cell, so every alias observes the others' writes.
    Ref(usize),
}

/// A scope's variables: one slot per name, reachable either by name or by the
/// index the compiler resolved for it.
///
/// Both routes address the SAME slot. That is the point — it is what lets
/// `extract()`, `$$name`, `unset()` and reference bindings stay coherent with
/// compiled slot access instead of needing a second, shadow representation that
/// could drift from this one.
#[derive(Default)]
struct Vars {
    slots: Vec<Slot>,
    index: FxHashMap<String, u32>,
}

impl Vars {
    /// The slot `name` already occupies, if it has one. A slot survives
    /// `unset()` (it just goes back to `Unset`), so an index handed out once
    /// stays valid for the life of the frame.
    fn slot_of(&self, name: &str) -> Option<u32> {
        self.index.get(name).copied()
    }

    /// The slot for `name`, reserving one if this is the first mention.
    fn ensure_slot(&mut self, name: &str) -> u32 {
        if let Some(&i) = self.index.get(name) {
            return i;
        }
        let i = self.slots.len() as u32;
        self.slots.push(Slot::Unset);
        self.index.insert(name.to_string(), i);
        i
    }

    fn at(&self, i: u32) -> &Slot {
        self.slots.get(i as usize).unwrap_or(&Slot::Unset)
    }

    fn put(&mut self, i: u32, v: Slot) {
        // Grow rather than drop: a frame that was not seeded from a compiled
        // slot order still behaves self-consistently instead of silently losing
        // the write.
        if self.slots.len() <= i as usize {
            self.slots.resize(i as usize + 1, Slot::Unset);
        }
        self.slots[i as usize] = v;
    }

    fn get(&self, name: &str) -> &Slot {
        match self.slot_of(name) {
            Some(i) => self.at(i),
            None => &Slot::Unset,
        }
    }

    /// Every bound name and its slot, unbound ones skipped — what
    /// `get_defined_vars()`, `compact()` and the debugger's locals view walk.
    /// Renumber so `names[i]` occupies slot `i`, keeping whatever each name is
    /// already bound to and keeping every other binding as well.
    ///
    /// The global frame has its superglobals bound before the main chunk is
    /// loaded, so simply reserving in order would push the chunk's own names
    /// past them and every index it was compiled with would be wrong.
    fn renumber(&mut self, names: &[String]) {
        let mut slots: Vec<Slot> = Vec::with_capacity(names.len() + self.slots.len());
        let mut index: FxHashMap<String, u32> = FxHashMap::default();
        for n in names {
            let cur = self.get(n).clone();
            index.insert(n.clone(), slots.len() as u32);
            slots.push(cur);
        }
        for (k, &i) in self.index.iter() {
            if index.contains_key(k) {
                continue;
            }
            let v = self.slots.get(i as usize).cloned().unwrap_or(Slot::Unset);
            index.insert(k.clone(), slots.len() as u32);
            slots.push(v);
        }
        self.slots = slots;
        self.index = index;
    }

    fn iter(&self) -> impl Iterator<Item = (&String, &Slot)> {
        self.index
            .iter()
            .filter_map(|(k, &i)| match self.slots.get(i as usize) {
                Some(Slot::Unset) | None => None,
                Some(s) => Some((k, s)),
            })
    }
}

/// One variable scope (the global scope, or a function-call frame).
#[derive(Default)]
struct Scope {
    vars: Vars,
    /// The source line this frame is currently executing. Under `--dap` every
    /// statement updates it; on the normal path only the ops that can start a
    /// call or raise a throw do (see `builtins::mark_frame_line`), which is all
    /// an exception backtrace needs — the frame's line at the moment it called
    /// deeper is exactly the call-site line PHP prints for the frame above it.
    line: u32,
    /// The function name for a call frame, `None` for the global scope. Reported
    /// as the frame name in a DAP `stackTrace`.
    name: Option<String>,
    /// The late-static-binding class of this frame — the class the call named,
    /// which `static::` resolves to. `None` outside a method call.
    static_class: Option<String>,
    /// Set only for a closure frame: where the literal was written, which is
    /// what PHP 8.4 names the frame by. Kept apart from `name`, which encodes
    /// the private-access scope and is parsed back for visibility checks.
    closure_site: Option<DeclSite>,
    /// Set for a frame that stands in for a LIBRARY function rather than PHP
    /// source. A trace prints the call site of the frame ABOVE such a frame as
    /// `[internal function]`, because there is no PHP line to name.
    internal: bool,
    /// The file this frame's code was compiled from, when it is not the main
    /// script: a function declared in an `include`d file, or a closure written
    /// in `eval()`'d code. Taken from the body chunk's `source` at the call.
    /// `None` for the main script's code — and, for an `internal` frame, "the
    /// caller's", which is where its diagnostics point.
    file: Option<std::sync::Arc<str>>,
    /// A frame the reference has no counterpart for: an iterator method a
    /// `foreach` runs where the reference walks the object through its
    /// native iterator. A trace leaves it out, and what it calls is reported
    /// as called from where IT was called.
    transparent: bool,
}

/// One running `include`/`require`/`eval`: the code it loaded executes on the
/// frame at `depth` (an index into the scope stack), so a stack trace shows it
/// as a frame of its own between that one and whatever it calls.
struct IncludeFrame {
    depth: usize,
    /// The file the loaded code came from — for `eval`, the synthetic
    /// `<file>(<line>) : eval()'d code` name the reference gives it.
    file: std::sync::Arc<str>,
    /// How a trace names the frame: `include('/path/to/f.php')`, `eval()`.
    label: String,
    /// The line of the construct in the code that reached it.
    call_line: u32,
}

/// Where a user function or class was declared — what a redeclaration quotes
/// as `(previously declared in <file>:<line>)`.
#[derive(Debug, Clone)]
pub struct DeclAt {
    pub file: String,
    pub line: u32,
    /// The namespace it was declared in. phplang resolves names flat, so a
    /// same-named declaration in ANOTHER namespace is not a redeclaration and
    /// is not diagnosed as one.
    pub namespace: String,
}

/// The PHP runtime state for one thread.
pub struct PhpHost {
    objs: Vec<PhpObj>,
    scopes: Vec<Scope>,
    functions: FxHashMap<String, FuncDef>,
    /// Declaration sites of the user functions, by lowercased name. A name
    /// with no entry and a definition is not the program's own.
    fn_sites: FxHashMap<String, DeclAt>,
    /// Declaration sites of the user classes, by lowercased name.
    class_sites: FxHashMap<String, DeclAt>,
    /// User-declared classes, keyed by lowercase class name.
    classes: FxHashMap<String, ClassDef>,
    /// When `Some`, `echo` appends here instead of writing to stdout (used by
    /// `eval_capture` and the test harness).
    capture: Option<String>,
    /// The output-buffering state of `main/output.c` — see
    /// [`crate::stdlib::output`]. When a handler is on the stack, `echo` appends
    /// to the top one instead of the capture buffer / stdout.
    pub(crate) ob: crate::stdlib::output::OutputState,
    error: Option<String>,
    signal: Option<Signal>,
    /// The in-flight exception object, if a `throw` has fired and not yet been
    /// caught. Kept apart from `signal` so it survives function-frame unwinding
    /// and bubbles through nested VMs on its own.
    pending_throw: Option<Value>,
    /// What `__debugInfo` answered for each object handle in the value a
    /// `var_dump` / `print_r` is rendering, collected before the render starts
    /// (calling PHP code needs the host unborrowed). Empty outside a dump.
    pub(crate) debug_info: FxHashMap<u32, Value>,
    /// What `__serialize` answered for each object handle in the value a
    /// `serialize` is rendering, collected the same way. Empty outside one.
    pub(crate) serialize_info: FxHashMap<u32, Value>,
    /// The status an `exit`/`die` asked the process to end with, once one has
    /// run. Like [`PhpHost::pending_throw`] it unwinds every frame, but it is
    /// NOT catchable and does NOT run a `finally` — PHP ends the request where
    /// the `exit` stands. Kept in its own field precisely so `catch (Throwable)`
    /// cannot see it.
    pub(crate) pending_exit: Option<i32>,
    /// `set_exception_handler`'s stack: the last entry is the live handler,
    /// `Value::Undef` one that `set_exception_handler(null)` pushed.
    exception_handlers: Vec<Value>,
    /// Whether an uncaught exception may reach [`PhpHost::exception_handlers`]
    /// at all — not under `php -r` (see [`run_main`]).
    exception_handler_enabled: bool,
    /// `set_error_handler`'s stack of `(callback, error_levels)`; the last
    /// entry is live, a `Value::Undef` callback one `set_error_handler(null)`
    /// pushed.
    error_handlers: Vec<(Value, i64)>,
    /// A user error handler is running.
    in_error_handler: bool,
    /// Diagnostics the live handler takes, waiting for [`drain_error_handlers`]:
    /// `(severity, level, message, file, line)`.
    queued_errors: Vec<(String, i64, String, String, u32)>,
    /// `register_shutdown_function`'s queue: each callable with its arguments,
    /// run in order at request end (see [`request_shutdown`]).
    shutdown_fns: Vec<(Value, Vec<Value>)>,
    /// Set while a diagnostic of the request shutdown is written: the
    /// reference has no script position then and names the file `Unknown`.
    unknown_site: bool,
    /// Objects whose `__destruct` the request-end sweep has already run, so
    /// neither pass runs it twice.
    destructed: FxHashSet<u32>,
    /// Set by a fatal error that is not an uncaught exception: the reference
    /// marks every object destructed then, so no `__destruct` runs at request end.
    destructors_off: bool,
    /// The main chunk's globals in first-appearance order, and which of them
    /// live in fusevm frame slots — see [`crate::compiler::Program`].
    main_order: Vec<String>,
    main_promoted: Vec<String>,
    /// The main chunk's fusevm frame slots as the run left them, captured
    /// before its VM is recycled so request shutdown can still see the
    /// promoted globals.
    main_frame_slots: Vec<Value>,
    /// Level of the most recent `break`/`continue` signal — see
    /// [`last_break_level`].
    last_break_level: u32,
    /// Compiled `try`/`catch`/`finally` constructs, indexed by the id the
    /// compiler bakes into each `RUN_TRY` call.
    try_defs: Vec<TryDef>,
    /// Named constants (`PHP_EOL`, `M_PI`, user `define`s), keyed case-sensitively.
    /// Seeded with the standard predefined constants on every fresh host.
    constants: FxHashMap<String, Value>,
    /// The next resource number to hand out. Numbers are never reused.
    next_resource_id: i64,
    /// The `include`/`require`/`eval` constructs currently running, innermost
    /// last. Each runs on the frame it was reached from rather than on one of
    /// its own, so it is recorded here instead of on the scope stack.
    includes: Vec<IncludeFrame>,
    /// Every file an `include`/`require` has loaded, resolved, in load order —
    /// `get_included_files()` and what the `_once` forms test against.
    included_files: Vec<String>,
    /// Where the next run-time compilation (`include`, `eval`) starts its
    /// per-compilation counters, so the names it mints never collide with those
    /// of the code already loaded.
    compile_counters: crate::compiler::Counters,
    /// Shared storage cells for reference bindings (`$b = &$a`). A scope's `refs`
    /// map points names at these slots.
    ref_cells: Vec<Value>,
    /// The most recent call's by-reference parameters' final values, indexed by
    /// parameter position — read by the caller's post-call write-back (`BYREF_OUT`).
    byref_out: Vec<Value>,
    /// Which positions of `byref_out` the last call actually took by reference.
    /// The call sites that cannot know their callee statically test this before
    /// writing anything back — see [`PhpHost::byref_out_live`].
    byref_live: Vec<bool>,
    /// Static-property storage, keyed by `"declaringclass::prop"` (lowercased
    /// class). One cell per declaring class is shared by every subclass and every
    /// instance, matching PHP's static-property semantics.
    static_props: FxHashMap<String, Value>,
    /// Persistent slots for function `static $var` locals, keyed by the
    /// compile-time-unique id the compiler bakes into each declaration. The value
    /// is an index into `ref_cells`, so a body's `$var` aliases the cell and its
    /// value survives across calls.
    static_slots: FxHashMap<String, usize>,
    /// Singleton `enum` case instances, keyed by `"enumlower::CaseName"`. Built
    /// once on first access so `E::Case === E::Case` holds by object identity.
    enum_case_cache: FxHashMap<String, Value>,
    /// What a diagnostic names as the source: the script path, or
    /// `"Command line code"` for `php -r`. Set once at startup.
    script_name: String,
    /// Whether this compilation unit declared `strict_types=1`.
    ///
    /// Upstream this is PER FILE and read from the CALLER's file, so a strict file
    /// calling a non-strict file's function still checks strictly. This engine has
    /// no `include`, so a run is exactly one file and the two readings coincide;
    /// the flag is therefore whole-program without being unfaithful.
    strict_types: bool,
    /// Nesting depth of `@expr` suppression regions; a warning raised while this
    /// is non-zero is discarded.
    suppress: usize,
    /// The late-static-binding class the next call's frame will take. Set from
    /// the called class name, or — for a forwarding call (`self::`, `parent::`,
    /// `static::`) — from the caller's own, and taken when the frame is pushed.
    pending_lsb: Option<String>,
    /// The closure-literal site the next pushed frame takes, set just before a
    /// closure body is entered. A one-shot like `pending_lsb`, because the frame
    /// is built inside `invoke_with_locals`, which every kind of call shares.
    pending_closure_site: Option<DeclSite>,
    /// Set while a `foreach` calls a method of an iterator the reference
    /// walks natively: the frame that call pushes is `transparent`.
    pending_transparent: bool,
    /// The reference cell the most recent `function &f()` return published, for
    /// the caller's `$r = &f()` to bind. Taken (not just read) by the binding so a
    /// stale slot can never be picked up by a later plain call.
    ret_ref_slot: Option<usize>,
    /// Live generators, indexed by the id a `PhpObj::Generator` handle carries.
    /// Each holds a suspended stackful coroutine plus its swapped-out execution
    /// context (its own call frame, in-flight signal/throw).
    generators: Vec<GenCell>,
    /// Set once a fatal error has been displayed in PHP's own shape (see
    /// [`PhpHost::fatal`]), so the CLI wrapper reports the exit status without
    /// printing the message a second time in its terse `php: …` form.
    fatal_reported: bool,
    /// The `error_reporting` mask: a diagnostic is displayed only when its `E_*`
    /// bit is set here. Seeded from `-d error_reporting=…` (or `E_ALL`), then
    /// writable at run time through `error_reporting()` / `ini_set`.
    error_reporting: i64,
    /// The last diagnostic raised, as `error_get_last()` reports it: `(level,
    /// message, file, line)`. Recorded whether or not it was displayed — `@`
    /// and `error_reporting` hide a diagnostic, they do not un-raise it.
    last_error: Option<(i64, String, String, u32)>,
    /// `ini_set`/`ini_get` string store for settings with no dedicated field, so a
    /// value written by one is read back by the other unchanged.
    ini: FxHashMap<String, String>,
    /// What `preg_last_error()` reports: the outcome of the last `preg_*` call
    /// that reached the regex compiler. Sticky — only another such call rewrites
    /// it, so `preg_last_error()` itself and `preg_quote()` leave it alone.
    preg_error: i64,
    /// `strtok`'s tokenizer position: the subject it was last handed and the byte
    /// offset the next call resumes from. Upstream this is the pair
    /// `BG(strtok_string)` / `BG(strtok_last)`; `None` is upstream's NULL
    /// `strtok_string`, which is both the never-started state and the state
    /// `strtok` resets to when it runs out of tokens.
    strtok_state: Option<(String, usize)>,
    /// The object whose `__clone()` is currently running, if any. A `readonly`
    /// property is writable exactly once, and PHP 8.3 reopens that one write
    /// inside `__clone` so a copy can be given a fresh identity — the only
    /// place a second write to an initialized readonly property is legal.
    cloning: Option<u32>,
    /// Per object, the readonly properties that have already taken their one
    /// write. Kept beside the objects rather than in them because "written" is
    /// not a property of the stored value — a readonly property holding null
    /// may be initialized or may never have been assigned, and only these two
    /// states tell the two refusal messages apart.
    readonly_init: FxHashMap<u32, FxHashSet<String>>,
    /// Typed properties `unset()` has removed, per object handle. An
    /// uninitialized typed property is read through `__get` only after an
    /// explicit `unset()`; before its first write it is an `Error` even when the
    /// class has `__get`, which is how lazy-initialization idioms tell the two
    /// apart.
    unset_typed: FxHashMap<u32, FxHashSet<String>>,
    /// Magic property accesses currently on the stack, as
    /// `(object handle, property, magic method)`. PHP does not re-enter a magic
    /// method for a property already being handled by it, which is what lets
    /// `__get($n) { return $this->$n ?? 'd'; }` terminate: the inner `$this->$n`
    /// finds no property, sees `__get` already in progress for that name, and
    /// takes the ordinary undefined-property path instead of recursing.
    magic_in_progress: Vec<(u32, String, &'static str)>,
    /// Classes whose constant or static-property INITIALIZER is running, innermost
    /// last. A constant expression is evaluated in the scope of the class that
    /// declares it, so `const R = self::P + 1;` may read a `private` `P` even when
    /// the first read of `R` comes from outside the class.
    init_class_scope: Vec<String>,
    /// Handles of objects a RENDERER allocated to stage a value (see
    /// [`PhpHost::new_transient_object`]). They are never reachable from PHP, so
    /// [`PhpHost::object_ordinal`] skips them — otherwise a `json_encode` would
    /// silently bump the `#n` that a later `var_dump` prints.
    transient_objs: FxHashSet<u32>,
}

/// What `$obj->m(...)` / `C::m(...)` should do, once the method table and the
/// visibility rules have both been consulted. Returned by
/// [`PhpHost::method_dispatch`], which every call opcode routes through so the
/// instance and static forms cannot disagree about when `__call` fires.
///
/// The order is the reference's, and the interesting arm is `Magic` for a
/// method that IS declared: PHP routes a call it cannot reach to `__call` just
/// as readily as one that does not exist, so a private method plus a `__call`
/// is handled by the magic method rather than reported as an access error.
pub enum MethodDispatch {
    /// Declared (somewhere up the chain) and reachable from here: call it.
    Direct,
    /// Undeclared, or declared but out of reach, and the class supplies the
    /// magic catch-all — call `__call`/`__callStatic` with the argument list.
    Magic,
    /// Declared, out of reach, and no magic catch-all. Carries the reference's
    /// `Call to <vis> method C::m() from <scope>` message.
    Denied(String),
    /// No such method anywhere up the chain and no magic catch-all.
    Undefined,
}

/// What `$obj->name` should do, once the object's property table and the
/// visibility rules have both been consulted. Returned by
/// [`PhpHost::prop_access`], which every property opcode routes through so the
/// four of them cannot disagree about when a magic method fires.
pub enum PropAccess {
    /// The property is present on the object and reachable from here: read,
    /// write or remove the slot directly.
    Direct,
    /// Not directly reachable, and the class defines the magic method for this
    /// operation — call it.
    Magic,
    /// Not reachable, no magic method, and the property IS declared: its
    /// visibility puts it out of reach. Carries the reference's message.
    Denied(String),
    /// Not reachable, no magic method, and no such property. What that means is
    /// the caller's decision: a read warns and yields null, a write creates the
    /// property, an unset does nothing, an `isset` is false.
    Absent,
}

/// Ini settings applied to every host the moment it is created — what `php -d
/// name=value` writes.
///
/// A process-wide cell rather than an argument to `PhpHost::new`: hosts are
/// created implicitly on first `with_host` and thrown away by every
/// `reset_host`, so there is no single call site that could pass them in, and a
/// value set once on a host would not survive the next reset.
static INITIAL_INI: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// Record a `-d name=value` override for every host created from now on.
pub fn add_initial_ini(name: &str, value: &str) {
    let mut g = INITIAL_INI.lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|(k, _)| k != name);
    g.push((name.to_string(), value.to_string()));
}

/// The ini settings this engine knows, with the values the reference CLI reports
/// for them when nothing overrides them.
///
/// Every entry here is an ENGINE default — compiled into the interpreter, not
/// read from a php.ini. The distinction is testable rather than a judgement
/// call: `php -n` starts with no ini file at all, so a setting whose value is
/// the same under `php -n` and `php` is one the engine supplies itself, and a
/// setting whose value differs between them came from the file. Only the former
/// are listed, which is why `memory_limit` (`128M`) and `date.timezone` (`UTC`)
/// belong here — both survive `php -n` unchanged — while `variables_order`,
/// `enable_dl` and `short_open_tag`, which do not, are deliberately absent.
///
/// Two further exclusions, for values that are engine defaults but not portable
/// ones: anything whose default is a build path (`extension_dir`,
/// `include_path`, `openssl.cafile`), which encodes the prefix the reference
/// happened to be built with, and settings belonging to optional extensions
/// (`mysqli.*`, `pgsql.*`, `tidy.*`), which a differently configured build does
/// not register at all. What remains is PHP core plus `date` and `pcre` — the
/// two extensions PHP 8 cannot be built without.
fn default_ini() -> FxHashMap<String, String> {
    [
        // The startup mask, also readable through `ini_get`. `-d` and
        // `error_reporting()` both rewrite this entry, so the two views agree.
        ("error_reporting", "30719"),
        // Error handling and output.
        ("precision", "14"),
        ("serialize_precision", "-1"),
        ("display_errors", "1"),
        ("display_startup_errors", "1"),
        ("log_errors", "1"),
        // Empty, not absent: with no php.ini the reference reports `string(0) ""`
        // for `error_log`, which is what sends the `log_errors` copy to stderr.
        ("error_log", ""),
        ("html_errors", "0"),
        ("docref_ext", ""),
        ("error_append_string", ""),
        ("error_prepend_string", ""),
        ("ignore_repeated_errors", "0"),
        ("ignore_repeated_source", "0"),
        ("report_memleaks", "1"),
        ("fatal_error_backtraces", "1"),
        ("output_buffering", "0"),
        ("implicit_flush", "1"),
        ("output_handler", ""),
        // Resource limits. `max_execution_time` and `max_input_time` are the CLI
        // SAPI's own hardcoded overrides, not the common defaults.
        ("memory_limit", "128M"),
        ("max_memory_limit", "-1"),
        ("max_execution_time", "0"),
        ("max_input_time", "-1"),
        ("max_input_nesting_level", "64"),
        ("max_input_vars", "1000"),
        ("post_max_size", "8M"),
        ("default_socket_timeout", "60"),
        ("hard_timeout", "2"),
        ("unserialize_max_depth", "4096"),
        ("unserialize_callback_func", ""),
        // Language and encoding.
        ("default_charset", "UTF-8"),
        ("default_mimetype", "text/html"),
        ("input_encoding", ""),
        ("internal_encoding", ""),
        ("output_encoding", ""),
        ("disable_functions", ""),
        ("expose_php", "1"),
        ("ignore_user_abort", "0"),
        ("register_argc_argv", "0"),
        ("auto_detect_line_endings", "0"),
        ("allow_url_fopen", "1"),
        ("arg_separator.input", "&"),
        ("arg_separator.output", "&"),
        ("user_agent", ""),
        ("from", ""),
        // Zend engine.
        ("zend.assertions", "1"),
        ("zend.enable_gc", "1"),
        ("zend.detect_unicode", "1"),
        ("zend.multibyte", "0"),
        ("zend.exception_ignore_args", "0"),
        ("zend.exception_string_param_max_len", "15"),
        ("zend.script_encoding", ""),
        // ext/date — always built.
        ("date.timezone", "UTC"),
        ("date.default_latitude", "31.7667"),
        ("date.default_longitude", "35.2333"),
        ("date.sunrise_zenith", "90.833333"),
        ("date.sunset_zenith", "90.833333"),
        // ext/pcre — always built.
        ("pcre.backtrack_limit", "1000000"),
        ("pcre.recursion_limit", "100000"),
        ("pcre.jit", "1"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

/// Settings from [`default_ini`] that `ini_set` cannot change at run time —
/// PHP's `PHP_INI_PERDIR`/`PHP_INI_SYSTEM` entries, which only a php.ini or a
/// `-d` may set. `ini_set` reports `false` for these and changes nothing, while
/// `ini_get` still reads them.
///
/// Determined by running `ini_set($name, ini_get($name))` on the reference for
/// every name in `default_ini` and recording which ones answered `false` —
/// writing a setting its OWN current value can only fail because the write
/// itself is refused.
const INI_FIXED: &[&str] = &[
    "allow_url_fopen",
    "arg_separator.input",
    "disable_functions",
    "expose_php",
    "hard_timeout",
    "max_input_nesting_level",
    "max_input_time",
    "max_input_vars",
    "max_memory_limit",
    "output_buffering",
    "output_handler",
    "post_max_size",
    "register_argc_argv",
    "zend.multibyte",
    "zend.script_encoding",
];

/// The set of array/object handles currently being walked by a recursive
/// renderer, so a structure that contains itself is DETECTED rather than
/// followed forever.
///
/// This is the analogue of the reference's `GC_PROTECT_RECURSION`: the engine
/// flags a hashtable while it is being printed and, on re-entering a flagged
/// one, substitutes a marker. Without it every one of these walkers exhausts
/// the native stack and the process aborts, which no PHP program can catch.
#[derive(Default)]
pub struct Visiting(Vec<u32>);

impl Visiting {
    /// Mark `v` as being walked. `false` means it already was — the caller must
    /// emit its recursion marker instead of descending.
    pub fn enter(&mut self, v: &Value) -> bool {
        let Value::Obj(id) = v else { return true };
        if self.0.contains(id) {
            return false;
        }
        self.0.push(*id);
        true
    }

    /// Finish walking the value the matching [`Visiting::enter`] admitted.
    pub fn leave(&mut self) {
        self.0.pop();
    }
}

/// A string as a stack trace renders it: cut to 15 BYTES, then escaped.
///
/// Port of `smart_str_append_escaped_truncated(…, 15)` plus
/// `smart_str_append_escaped` (`Zend/zend_smart_str.c`). Every byte below 32,
/// the backslash itself, and every byte above 126 is escaped — the named ones
/// (`\n`, `\r`, `\t`, `\f`, `\v`, `\e`, `\\`) by their letter, the rest as
/// `\xHH` with UPPERCASE hex digits. A single quote is NOT escaped, even though
/// the result is wrapped in single quotes.
///
/// The truncation is by byte and happens BEFORE escaping, so a 15-byte cut can
/// land mid-character; the escaping then renders the orphaned bytes as `\xHH`,
/// which is why the reference never emits invalid UTF-8 here.
fn escape_trace_string(s: &str) -> String {
    const VK_ESCAPE: u8 = 0x1b;
    let bytes = s.as_bytes();
    let cut = bytes.len().min(15);
    let mut out = String::new();
    for &c in &bytes[..cut] {
        if c < 32 || c == b'\\' || c > 126 {
            out.push('\\');
            match c {
                b'\n' => out.push('n'),
                b'\r' => out.push('r'),
                b'\t' => out.push('t'),
                0x0c => out.push('f'),
                0x0b => out.push('v'),
                b'\\' => out.push('\\'),
                VK_ESCAPE => out.push('e'),
                _ => out.push_str(&format!("x{c:02X}")),
            }
        } else {
            out.push(c as char);
        }
    }
    if bytes.len() > 15 {
        out.push_str("...");
    }
    out
}

/// The message PHP raises when `$a[] =` cannot pick a key because the array
/// already holds an element at `PHP_INT_MAX`. Thrown as a plain `Error`, so
/// user code can catch it; the reference does not name a function in it.
pub const NEXT_ELEMENT_OCCUPIED: &str =
    "Cannot add element to the array as the next element is already occupied";

// DIVERGENCE — per-setting VALUE validation is not modelled. The reference
// refuses a value a setting will not take, with a `Warning` and `false`:
// `ini_set('date.timezone', '-1')` keeps UTC, `ini_set('memory_limit', '20')`
// keeps 128M. Neither is reproducible here. The timezone check needs a zone
// database this build does not carry (see `stdlib::datetime`, which documents
// the same gap for `date_default_timezone_set`), and `memory_limit`'s refusal
// message quotes the process's CURRENT memory usage in bytes, which is not a
// reproducible number. A write of a value the reference would refuse is
// therefore accepted here rather than guessed at; see the `ini_set` corpus entry.

impl Default for PhpHost {
    fn default() -> Self {
        Self::new()
    }
}

impl PhpHost {
    pub fn new() -> Self {
        let mut h = PhpHost {
            objs: Vec::new(),
            // Start with the global scope already open.
            scopes: vec![Scope::default()],
            functions: FxHashMap::default(),
            fn_sites: FxHashMap::default(),
            class_sites: FxHashMap::default(),
            classes: FxHashMap::default(),
            capture: None,
            error: None,
            signal: None,
            pending_throw: None,
            debug_info: FxHashMap::default(),
            serialize_info: FxHashMap::default(),
            pending_exit: None,
            exception_handlers: Vec::new(),
            error_handlers: Vec::new(),
            in_error_handler: false,
            queued_errors: Vec::new(),
            exception_handler_enabled: true,
            shutdown_fns: Vec::new(),
            unknown_site: false,
            destructed: Default::default(),
            destructors_off: false,
            main_order: Vec::new(),
            main_promoted: Vec::new(),
            main_frame_slots: Vec::new(),
            last_break_level: 1,
            try_defs: Vec::new(),
            constants: predefined_constants(),
            next_resource_id: 1,
            includes: Vec::new(),
            included_files: Vec::new(),
            compile_counters: crate::compiler::Counters::default(),
            ob: Default::default(),
            ref_cells: Vec::new(),
            byref_out: Vec::new(),
            byref_live: Vec::new(),
            static_props: FxHashMap::default(),
            static_slots: FxHashMap::default(),
            enum_case_cache: FxHashMap::default(),
            script_name: "Command line code".to_string(),
            suppress: 0,
            pending_lsb: None,
            pending_closure_site: None,
            pending_transparent: false,
            ret_ref_slot: None,
            generators: Vec::new(),
            fatal_reported: false,
            error_reporting: errlevel::E_ALL,
            last_error: None,
            ini: default_ini(),
            preg_error: 0,
            strtok_state: None,
            cloning: None,
            readonly_init: FxHashMap::default(),
            unset_typed: FxHashMap::default(),
            magic_in_progress: Vec::new(),
            init_class_scope: Vec::new(),
            transient_objs: FxHashSet::default(),
            strict_types: false,
        };
        h.init_superglobals();
        h.init_stdio();
        // `-d` overrides land before the program is read, so a compile-time
        // notice is already tested against the level they set.
        for (name, value) in INITIAL_INI.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            if name == "error_reporting" {
                // The ini path — unlike `ini_set` — runs the constant-expression
                // scanner, so `-d 'error_reporting=E_ALL & ~E_NOTICE'` works. The
                // RESOLVED number is what `ini_get` reports back afterwards.
                h.set_error_reporting(crate::errlevel::parse_ini_level(value).unwrap_or(0));
            } else {
                // Unlike `ini_set`, `-d` may introduce a name the engine has no
                // default for — the reference CLI registers it either way.
                h.ini.insert(name.clone(), value.clone());
            }
        }
        h
    }

    /// The last call's by-reference parameter value at `pos` (for the caller's
    /// write-back); `Undef` if out of range.
    pub fn byref_out_get(&self, pos: usize) -> Value {
        self.byref_out.get(pos).cloned().unwrap_or(Value::Undef)
    }

    /// Whether the call that just returned had a by-reference parameter at `pos`.
    ///
    /// A call site that cannot know the callee statically — a method call, whose
    /// receiver's class is a run-time fact, or `$f(…)` on a variable holding any
    /// callable — asks this before writing anything back. Without it a write-back
    /// emitted "just in case" would store the previous call's leftovers, or a
    /// null, into a variable the callee never took by reference.
    pub fn byref_out_live(&self, pos: usize) -> bool {
        self.byref_live.get(pos).copied().unwrap_or(false)
    }

    /// Record which of a returning call's parameters were by-reference, and their
    /// final values. Positions the callee did not take by reference are cleared,
    /// so nothing survives from the call before.
    pub fn byref_out_set(&mut self, vals: Vec<Value>, live: Vec<bool>) {
        self.byref_out = vals;
        self.byref_live = live;
    }

    /// Publish one by-reference OUT value from a *builtin* — `preg_match`'s
    /// `$matches`, `parse_str`'s result array. A user function fills the whole
    /// vector from its frame when it returns; a builtin has no frame, so it
    /// names the position it is writing.
    pub fn byref_out_put(&mut self, pos: usize, v: Value) {
        if self.byref_out.len() <= pos {
            self.byref_out.resize(pos + 1, Value::Undef);
            self.byref_live.resize(pos + 1, false);
        }
        self.byref_out[pos] = v;
        self.byref_live[pos] = true;
    }

    /// Drop the previous call's OUT values, so a call that writes none cannot
    /// be read as having written the one before it.
    pub fn byref_out_clear(&mut self) {
        self.byref_out.clear();
        self.byref_live.clear();
    }

    /// Seed the superglobal arrays in the global scope: `$_ENV`/`$_SERVER` from
    /// the real process environment, empty `$_GET`/`$_POST`/… request arrays, and
    /// `$argv`/`$argc`. Visible from every scope via `is_superglobal` resolution.
    /// The CLI's standard streams: `STDIN`, `STDOUT` and `STDERR` are
    /// resources 1, 2 and 3. The reference's first stream a script opens is 5,
    /// so the counter resumes there.
    fn init_stdio(&mut self) {
        use crate::stdlib::fileres::{Stream, StreamKind};
        for (name, kind) in [
            ("STDIN", StreamKind::Stdin),
            ("STDOUT", StreamKind::Stdout),
            ("STDERR", StreamKind::Stderr),
        ] {
            let v = self.new_stream(Stream::stdio(kind));
            self.constants.insert(name.to_string(), v);
        }
        self.next_resource_id = 5;
    }

    fn init_superglobals(&mut self) {
        let env: Vec<(String, String)> = std::env::vars().collect();
        let mkenv = |h: &mut PhpHost| -> Value {
            let a = h.new_array();
            for (k, v) in &env {
                h.arr_set_key(&a, &Value::str(k.clone()), Value::str(v.clone()));
            }
            a
        };
        let e = mkenv(self);
        self.set_var("_ENV", e);
        let s = mkenv(self);
        // A few conventional `$_SERVER` entries on top of the environment.
        self.arr_set_key(&s, &Value::str("PHP_SELF"), Value::str(String::new()));
        self.arr_set_key(&s, &Value::str("SCRIPT_NAME"), Value::str(String::new()));
        // Empty rather than a stand-in name when there is no file, which is what
        // the reference reports for `php -r` and for a script on stdin.
        self.arr_set_key(
            &s,
            &Value::str("SCRIPT_FILENAME"),
            Value::str(String::new()),
        );
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        self.arr_set_key(&s, &Value::str("REQUEST_TIME"), Value::int(now));
        self.set_var("_SERVER", s);
        for name in [
            "_GET", "_POST", "_REQUEST", "_COOKIE", "_FILES", "_SESSION", "GLOBALS",
        ] {
            let empty = self.new_array();
            self.set_var(name, empty);
        }
        let argv = self.new_array();
        self.arr_push_auto(&argv, Value::str(String::new()));
        self.set_var("argv", argv);
        self.set_var("argc", Value::int(1));
    }

    /// Publish the command line into the superglobals the reference fills from
    /// it: `$argv`/`$argc`, their `$_SERVER` copies, and the three `$_SERVER`
    /// names that report the script.
    ///
    /// `script` is the `FILE` argument VERBATIM, not the resolved path
    /// [`PhpHost::script_name`] carries — the reference keeps the two apart, so
    /// `php sub/s.php` has `$argv[0] === "sub/s.php"` while `__FILE__` is the
    /// absolute path. Code with no file reports `Standard input code`, and it
    /// reports that for `php -r` too, where `__FILE__` says `Command line code`
    /// instead; the two names genuinely disagree there.
    ///
    /// `SCRIPT_FILENAME` is the one that does NOT fall back to a stand-in name:
    /// with no file (`file` is `None`) it stays the empty string.
    pub fn set_script_args(&mut self, file: Option<&str>, args: &[String]) {
        let script = file.unwrap_or("Standard input code").to_string();
        let argv = self.new_array();
        self.arr_push_auto(&argv, Value::str(script.clone()));
        for a in args {
            self.arr_push_auto(&argv, Value::str(a.clone()));
        }
        let argc = Value::int(args.len() as i64 + 1);
        self.set_var("argv", argv.clone());
        self.set_var("argc", argc.clone());
        let server = self.get_var("_SERVER");
        for (k, v) in [
            ("argv", argv),
            ("argc", argc),
            ("PHP_SELF", Value::str(script.clone())),
            ("SCRIPT_NAME", Value::str(script)),
            (
                "SCRIPT_FILENAME",
                Value::str(file.unwrap_or_default().to_string()),
            ),
        ] {
            self.arr_set_key(&server, &Value::str(k.to_string()), v);
        }
    }

    /// A snapshot of all defined constants as `(name, value)` pairs — for
    /// `get_defined_constants`.
    pub fn all_constants(&self) -> Vec<(String, Value)> {
        self.constants
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// The names of all declared classes (lowercased, as stored) — for
    /// `get_declared_classes`.
    pub fn all_class_names(&self) -> Vec<String> {
        self.classes.keys().cloned().collect()
    }

    // ── constants ───────────────────────────────────────────────────────────

    /// `constant(name)` / a bare constant reference, or `None` when no constant
    /// of that name is defined.
    ///
    /// An undefined one is an `Error` in PHP 8, not the bare name as a string —
    /// that leniency was PHP 7's and was removed. Raising it needs a throw, which
    /// this cannot do, so both callers turn the `None` into one themselves.
    pub fn const_fetch(&self, name: &str) -> Option<Value> {
        self.constants.get(name).cloned()
    }

    /// Whether a constant of this name is defined.
    pub fn const_defined(&self, name: &str) -> bool {
        self.constants.contains_key(name)
    }

    /// Defines a constant, returning `true` unless it was already defined — PHP
    /// does not redefine, and the FIRST value is the one that survives.
    ///
    /// A redefinition is not silent: it warns. Both spellings that reach here
    /// (`define()` and the `const` declaration) warn identically, which is why
    /// the warning lives at this single write point rather than at either call
    /// site.
    pub fn const_define(&mut self, name: &str, value: Value) -> bool {
        if self.constants.contains_key(name) {
            self.warn(format!(
                "Constant {name} already defined, this will be an error in PHP 9"
            ));
            return false;
        }
        self.constants.insert(name.to_string(), value);
        true
    }

    // ── program loading ────────────────────────────────────────────────────

    /// Install a compiled program's user functions onto the host.
    pub fn load_program(&mut self, functions: Vec<(String, FuncDef)>) {
        for (name, def) in functions {
            self.functions.insert(name, def);
        }
    }

    /// Install a compiled program's classes onto the host.
    pub fn load_classes(&mut self, classes: Vec<(String, ClassDef)>) {
        for (name, def) in classes {
            self.classes.insert(name, def);
        }
    }

    /// Record where a loaded program's functions and classes were declared:
    /// `(lowercased name, line, namespace)` each, all in `file`.
    pub fn record_decl_sites(
        &mut self,
        file: &str,
        fns: Vec<(String, u32, String)>,
        classes: Vec<(String, u32, String)>,
    ) {
        let at = |line, namespace| DeclAt {
            file: file.to_string(),
            line,
            namespace,
        };
        for (name, line, ns) in fns {
            self.fn_sites.insert(name, at(line, ns));
        }
        for (name, line, ns) in classes {
            self.class_sites.insert(name, at(line, ns));
        }
    }

    /// The body of the reference's `Cannot redeclare function …` for declaring
    /// `name` in namespace `ns`, or `None` when the name is free.
    ///
    /// A user function is quoted with the site it was declared at; a library
    /// function — or one of the PHP-written prelude's — has none to quote. A
    /// namespaced declaration never collides with a library function, which
    /// lives in the global namespace.
    pub fn fn_redeclare_msg(&self, name: &str, ns: &str) -> Option<String> {
        let lname = name.to_ascii_lowercase();
        match self.fn_sites.get(&lname) {
            Some(at) if at.namespace == ns => Some(format!(
                "Cannot redeclare function {name}() (previously declared in {}:{})",
                at.file, at.line
            )),
            Some(_) => None,
            None if ns.is_empty()
                && (self.functions.contains_key(&lname)
                    || crate::stdlib::callable::is_library_function(&lname)) =>
            {
                Some(format!("Cannot redeclare function {name}()"))
            }
            None => None,
        }
    }

    /// The body of the reference's `Cannot redeclare <kind> <Name> …` for
    /// declaring a class, interface, trait or enum called `name` in namespace
    /// `ns`, or `None` when the name is free. The message names the type that
    /// is ALREADY declared — its kind and its spelling — not the new one.
    pub fn class_redeclare_msg(&self, name: &str, ns: &str) -> Option<String> {
        let lname = name.to_ascii_lowercase();
        if let Some(at) = self.class_sites.get(&lname) {
            if at.namespace != ns {
                return None;
            }
            let def = self.classes.get(&lname)?;
            return Some(format!(
                "Cannot redeclare {} {} (previously declared in {}:{})",
                class_kind_word(def),
                def.name,
                at.file,
                at.line
            ));
        }
        if !ns.is_empty() {
            return None;
        }
        if let Some(def) = self.classes.get(&lname) {
            return Some(format!(
                "Cannot redeclare {} {}",
                class_kind_word(def),
                def.name
            ));
        }
        if let Some((spelled, kind)) = crate::prelude_type(&lname) {
            return Some(format!("Cannot redeclare {kind} {spelled}"));
        }
        builtin_type(&lname).map(|(spelled, kind, ..)| {
            format!("Cannot redeclare {} {spelled}", type_kind_word(*kind))
        })
    }

    /// Declare a function whose definition was compiled under `key` (see
    /// `ops::DECLARE_FN`) as `name`, at `line` of the running file. `Err` is the
    /// redeclaration the reference refuses.
    pub fn declare_fn_at(
        &mut self,
        key: &str,
        name: &str,
        ns: &str,
        line: u32,
    ) -> Result<(), String> {
        if let Some(msg) = self.fn_redeclare_msg(name, ns) {
            return Err(msg);
        }
        let lname = name.to_ascii_lowercase();
        if let Some(def) = self.functions.remove(key) {
            self.functions.insert(lname.clone(), def);
        }
        let file = self.current_file().to_string();
        self.fn_sites.insert(
            lname,
            DeclAt {
                file,
                line,
                namespace: ns.to_string(),
            },
        );
        Ok(())
    }

    /// The `final` rule the class `class` breaks against the classes it
    /// inherits from, if any — see [`final_violation`].
    pub fn final_check(&self, class: &str) -> Option<(String, Option<u32>)> {
        let def = self.classes.get(&class.to_ascii_lowercase())?;
        final_violation(def, |n| self.classes.get(&n.to_ascii_lowercase()))
    }

    /// [`Self::final_check`] for `def` while it is still being compiled: it and
    /// the types compiled alongside it are not in the class table yet, so
    /// `local` is asked first.
    pub fn final_check_with<'a>(
        &'a self,
        def: &ClassDef,
        local: impl Fn(&str) -> Option<&'a ClassDef>,
    ) -> Option<(String, Option<u32>)> {
        final_violation(def, |n| {
            local(n).or_else(|| self.classes.get(&n.to_ascii_lowercase()))
        })
    }

    /// [`Self::declare_fn_at`] for a class compiled under `key`.
    pub fn declare_class_at(&mut self, key: &str, ns: &str, line: u32) -> Result<(), String> {
        let Some(def) = self.classes.get(key) else {
            return Ok(());
        };
        let name = def.name.clone();
        if let Some(msg) = self.class_redeclare_msg(&name, ns) {
            return Err(msg);
        }
        let lname = name.to_ascii_lowercase();
        if let Some(def) = self.classes.remove(key) {
            self.classes.insert(lname.clone(), def);
        }
        let file = self.current_file().to_string();
        self.class_sites.insert(
            lname,
            DeclAt {
                file,
                line,
                namespace: ns.to_string(),
            },
        );
        Ok(())
    }

    /// Install a program's compiled `try`/`catch`/`finally` table. The `RUN_TRY`
    /// ids the main chunk carries are offsets into this vector.
    pub fn load_try_defs(&mut self, defs: Vec<TryDef>) {
        self.try_defs = defs;
    }

    /// Whether the object class `class` is caught by `catch (target)`: the same
    /// class, a subclass (walking the parent chain), or `target == Throwable`
    /// (the interface both exception roots implement). Names are compared
    /// case-insensitively, matching PHP and the lowercased class table.
    pub fn catch_matches(&self, class: &str, target: &str) -> bool {
        let target = target.to_ascii_lowercase();
        if target == "throwable" {
            return self.class_is_a(class, "exception") || self.class_is_a(class, "error");
        }
        self.class_is_a(class, &target)
    }

    /// Whether `class` is `ancestor` or descends from it, walking the compiled
    /// class table's parent chain. `ancestor` must already be lowercased.
    /// `PhpHost::class_is_a` for callers outside this module (the call opcodes
    /// need it to decide whether a static call's `$this` belongs to the class).
    pub fn class_is_a_pub(&self, class: &str, ancestor: &str) -> bool {
        self.class_is_a(class, &ancestor.to_ascii_lowercase())
    }

    fn class_is_a(&self, class: &str, ancestor: &str) -> bool {
        // `Stringable` is implemented AUTOMATICALLY by any class with a
        // `__toString`, whether or not it names the interface (PHP 8.0). No other
        // interface behaves this way, so it is answered before the walk.
        if ancestor == "stringable" && self.class_has_method(class, "__tostring") {
            return true;
        }
        // Traverse the parent chain AND implemented/extended interfaces
        // (transitively). A visited set + bound guards against malformed cycles.
        let mut stack = vec![class.to_ascii_lowercase()];
        let mut seen: Vec<String> = Vec::new();
        while let Some(c) = stack.pop() {
            if c == ancestor {
                return true;
            }
            if seen.contains(&c) || seen.len() > 1000 {
                continue;
            }
            seen.push(c.clone());
            if let Some(d) = self.classes.get(&c) {
                if let Some(p) = &d.parent {
                    stack.push(p.to_ascii_lowercase());
                }
                for i in &d.interfaces {
                    stack.push(i.to_ascii_lowercase());
                }
            } else {
                // Not in the class table: it may still be one of the engine's
                // own types, whose ancestry lives in `BUILTIN_TYPES` — that is
                // what makes `$gen instanceof Traversable` true.
                stack.extend(
                    builtin_type_parents(&c)
                        .iter()
                        .map(|p| p.to_ascii_lowercase()),
                );
            }
        }
        false
    }

    // ── output capture ─────────────────────────────────────────────────────

    pub fn begin_capture(&mut self) {
        self.capture = Some(String::new());
    }

    pub fn end_capture(&mut self) -> String {
        self.capture.take().unwrap_or_default()
    }

    /// Write to the process's stderr, first flushing what stdout holds. The
    /// reference's CLI writes each stdout chunk as it is produced, so a line
    /// still sitting in Rust's line buffer would otherwise land AFTER a later
    /// diagnostic or `fwrite(STDERR, …)` on a shared `2>&1` stream.
    pub fn write_err(bytes: &[u8]) {
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().write_all(bytes);
    }

    /// A buffered write — `php_output_write`: into the output-buffering stack
    /// when a handler is active (see [`crate::stdlib::output`]), else the
    /// capture buffer if active, else stdout.
    pub fn write_out(&mut self, s: &str) {
        let top = self.ob.handlers.len();
        self.ob_write_below(top, s);
    }

    /// `php_output_op(PHP_OUTPUT_HANDLER_WRITE)` entered at the handler BELOW
    /// stack index `n` — `n` is the stack length for an ordinary write, and a
    /// handler's own index when its output is passed down. A disabled handler
    /// passes the data on; the first enabled one stores it, and one whose
    /// `chunk_size` it fills is due to run its handler, which happens at the
    /// next builtin boundary (`output::drain_chunks`) because a user handler
    /// is PHP code and this runs inside the host borrow.
    pub(crate) fn ob_write_below(&mut self, n: usize, s: &str) {
        if s.is_empty() {
            return;
        }
        let running = self.ob.running.is_some();
        for i in (0..n).rev() {
            let h = &mut self.ob.handlers[i];
            if h.is_disabled() {
                continue;
            }
            if running {
                h.flags |= crate::stdlib::output::PRODUCED_OUTPUT;
            }
            h.append(s);
            if h.chunk_full() && !running {
                self.ob.chunk_due = Some(h.id);
                crate::stdlib::output::mark_chunk_due();
            }
            return;
        }
        self.write_unbuffered(s);
    }

    /// Past every output handler: the capture buffer, or stdout.
    fn write_unbuffered(&mut self, s: &str) {
        if let Some(buf) = &mut self.capture {
            buf.push_str(s);
        } else {
            use std::io::Write;
            let mut out = std::io::stdout();
            let _ = out.write_all(s.as_bytes());
        }
    }

    // ── errors ─────────────────────────────────────────────────────────────

    // ── diagnostics ────────────────────────────────────────────────────────

    /// Name this run's source for diagnostics: the script path, or
    /// `"Command line code"` for `php -r` (which is the default).
    pub fn set_script_name(&mut self, name: impl Into<String>) {
        self.script_name = name.into();
        // A script that is a FILE is the first of `get_included_files()`, and
        // an `include_once` of it is a repeat.
        self.included_files = if self.script_name.starts_with('/') {
            vec![self.script_name.clone()]
        } else {
            Vec::new()
        };
    }

    /// What a diagnostic names as the source — the script path, or
    /// `"Command line code"` for `php -r`.
    pub fn script_name(&self) -> &str {
        &self.script_name
    }

    /// The file the code at scope `i` belongs to, ignoring any include running
    /// on it: the frame's own file, the caller's for an internal frame, the
    /// main script's otherwise.
    fn scope_file(&self, i: usize) -> &str {
        let scope = &self.scopes[i];
        if scope.internal && i > 0 {
            return self.scope_file(i - 1);
        }
        scope.file.as_deref().unwrap_or(&self.script_name)
    }

    /// The file the code running now belongs to — what a diagnostic, an
    /// exception's `getFile()`, `__FILE__` and `__DIR__` name. The innermost
    /// `include`/`eval` on the current frame wins over the frame's own file.
    pub fn current_file(&self) -> &str {
        if self.unknown_site {
            return "Unknown";
        }
        let top = self.scopes.len() - 1;
        match self.includes.last() {
            Some(f) if f.depth == top => &f.file,
            _ => {
                // An internal frame pushed over an include inherits it too.
                let mut i = top;
                while self.scopes[i].internal && i > 0 {
                    i -= 1;
                }
                match self.includes.last() {
                    Some(f) if f.depth == i => &f.file,
                    _ => self.scope_file(top),
                }
            }
        }
    }

    /// The line the innermost frame is executing — where an exception created
    /// right now records itself as having been raised, and the call site the
    /// frame below it reports in a backtrace.
    pub fn cur_frame_line(&self) -> u32 {
        self.scopes.last().map(|s| s.line).unwrap_or(0)
    }

    /// Enter/leave an `@expr` suppression region.
    pub fn suppress_push(&mut self) {
        self.suppress += 1;
    }

    pub fn suppress_pop(&mut self) {
        self.suppress = self.suppress.saturating_sub(1);
    }

    /// The open `@expr` region count, for a caller that has to put it back.
    pub fn suppress_depth(&self) -> usize {
        self.suppress
    }

    /// Force the region count back to `depth` — see the call in
    /// [`run_try_orchestrator`], the one place an `@expr` can be abandoned
    /// part-way through.
    pub fn suppress_restore(&mut self, depth: usize) {
        self.suppress = depth;
    }

    /// Emit a PHP `Warning` on the output stream.
    ///
    /// With the CLI defaults (`display_errors=STDOUT`, `html_errors=Off`) the
    /// reference interpreter writes exactly
    /// `"\nWarning: {msg} in {file} on line {n}\n"` to stdout, interleaved with
    /// the script's own output — so it goes through `write_out` and lands in an
    /// output buffer or a capture just as `echo` does. The copy `log_errors`
    /// sends to stderr is not reproduced: nothing observes it.
    pub fn warn(&mut self, msg: impl std::fmt::Display) {
        self.diagnose("Warning", errlevel::E_WARNING, warn_line(), msg);
    }

    /// Record whether this run declared `strict_types=1`. Set once, from the
    /// parse, before the program runs.
    pub fn set_strict_types(&mut self, on: bool) {
        self.strict_types = on;
    }

    /// Whether this run declared `strict_types=1`.
    pub fn strict_types(&self) -> bool {
        self.strict_types
    }

    /// How a value's type reads in a `TypeError`: PHP names the four scalars and
    /// `null` in lower case, an array as `array`, and an object by its CLASS.
    ///
    /// This is NOT [`crate::stdlib::types::debug_type`], and the two disagree on
    /// exactly one type. A diagnostic names a boolean by its VALUE — `true given`
    /// / `false given` — while `get_debug_type()` answers `"bool"` for both. PHP
    /// keeps them in separate functions (`zend_zval_value_name` against
    /// `zend_zval_get_legacy_type`/`get_debug_type`) for that reason, so any
    /// message of the form `… , <type> given` has to come from HERE. Reaching for
    /// `debug_type` because it is the public one silently writes `bool given`,
    /// which no PHP has ever printed.
    pub(crate) fn type_name_for_error(&self, v: &Value) -> String {
        match v {
            Value::Undef => "null".to_string(),
            Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            Value::Int(_) => "int".to_string(),
            Value::Float(_) => "float".to_string(),
            Value::Str(_) => "string".to_string(),
            Value::Obj(_) => match self.as_array(v) {
                Some(PhpObj::Array { .. }) => "array".to_string(),
                Some(PhpObj::Resource(_)) => "resource".to_string(),
                _ => self
                    .instance_class(v)
                    .map(|c| display_class(&c).to_string())
                    .unwrap_or_else(|| "object".to_string()),
            },
            _ => "mixed".to_string(),
        }
    }

    /// A `Warning` about a specific source line rather than the one the runtime
    /// last touched — used for the compile-time diagnostics, which are issued
    /// before any statement has run and so have no "current" line to borrow.
    pub fn warn_at(&mut self, msg: impl std::fmt::Display, line: u32) {
        self.diagnose("Warning", errlevel::E_WARNING, line, msg);
    }

    /// Emit a PHP `Deprecated` diagnostic. Same stream and shape as `warn`,
    /// which is the only thing that distinguishes the severities on output.
    pub fn deprecated(&mut self, msg: impl std::fmt::Display) {
        self.diagnose("Deprecated", errlevel::E_DEPRECATED, warn_line(), msg);
    }

    /// `(int) $v` / `intval($v)`: the explicit int cast, with the reference's
    /// diagnostic for a double no `int` can hold.
    ///
    /// PHP 8.1 added `The float %s is not representable as an int, cast
    /// occurred` for exactly the inputs [`dval_to_lval`] has to wrap — anything
    /// outside ±2^63, plus NAN and the infinities. A merely FRACTIONAL double
    /// is silent here; its `Implicit conversion … loses precision` deprecation
    /// belongs to the implicit contexts (array offsets, parameter binds), not
    /// to a cast the program asked for.
    pub fn to_int_cast(&mut self, v: &Value) -> i64 {
        // Only a FLOAT operand wraps. A numeric STRING written in float form
        // goes through `ZEND_STRTOL`, which saturates and says nothing:
        // `(int) 1e19` is -8446744073709551616 but `(int) "1e19"` is
        // `PHP_INT_MAX`.
        let Value::Float(f) = *v else {
            return self.to_number(v).to_int();
        };
        const TWO_63: f64 = 9223372036854775808.0;
        if !f.is_finite() || !(-TWO_63..TWO_63).contains(&f) {
            let shown = self.to_str(&Value::Float(f));
            self.warn(format!(
                "The float {shown} is not representable as an int, cast occurred"
            ));
        }
        dval_to_lval(f)
    }

    /// The diagnostic PHP issues when a value is USED as an array offset.
    ///
    /// Every offset use reports — reads and writes, and `isset`/`empty`/`unset`
    /// /`array_key_exists` alike — because the report is about the conversion,
    /// not about whether the element was found. Only two operand shapes convert
    /// lossily: `null` (which becomes `""`, not `0`) and a double that is
    /// fractional or too large for an `int`.
    ///
    /// It lives beside the mutating offset entry points rather than inside
    /// `norm_key`, which also normalizes keys taken back OUT of an
    /// array during a rebuild — those are already `Int`/`Str` and fall through
    /// here, so a rebuild cannot re-report the float key the array was built
    /// with. A STRING receiver is left alone: its own diagnostic is the
    /// differently worded `String offset cast occurred`.
    pub fn diagnose_offset(&mut self, key: &Value) {
        const TWO_63: f64 = 9223372036854775808.0;
        match key {
            Value::Undef => self.deprecated(
                "Using null as an array offset is deprecated, use an empty string instead",
            ),
            Value::Float(f) => {
                let (f, shown) = (*f, self.to_str(key));
                if !f.is_finite() || !(-TWO_63..TWO_63).contains(&f) {
                    self.warn(format!(
                        "The float {shown} is not representable as an int, cast occurred"
                    ));
                } else if f.fract() != 0.0 {
                    self.deprecated(format!(
                        "Implicit conversion from float {shown} to int loses precision"
                    ));
                }
            }
            Value::Obj(_) => {
                if let Some((id, _)) = self.resource_info(key) {
                    self.warn(format!(
                        "Resource ID#{id} used as offset, casting to integer ({id})"
                    ));
                }
            }
            _ => {}
        }
    }

    /// [`PhpHost::diagnose_offset`], but only for a receiver that really is an
    /// array — a string offset reports something else entirely.
    /// The name the reference gives an offset that cannot BE an offset — an
    /// array or an object — or `None` for every key it accepts.
    ///
    /// PHP has no coercion for these two: `$a[[1]]` and `$a[new C]` are a
    /// `TypeError`, not the `Array to string conversion` warning plus a lookup
    /// under the key `"Array"` that this engine used to perform.
    pub fn bad_offset_type(&self, key: &Value) -> Option<String> {
        if self.is_array(key) {
            return Some("array".to_string());
        }
        self.instance_class(key)
            .map(|c| display_class(&c).to_string())
    }

    pub fn diagnose_array_offset(&mut self, recv: &Value, key: &Value) {
        if matches!(self.as_array(recv), Some(PhpObj::Array { .. })) {
            self.diagnose_offset(key);
        }
    }

    /// [`PhpHost::diagnose_offset`] for a QUIET subscript — `isset($x[k])`,
    /// `empty($x[k])`, `$x[k] ?? $d`.
    ///
    /// A string receiver reports here too, and reports the ordinary narrowing
    /// deprecation rather than the `String offset cast occurred` warning its
    /// value-context read emits.
    pub fn diagnose_quiet_offset(&mut self, recv: &Value, key: &Value) {
        if matches!(recv, Value::Str(_)) {
            self.diagnose_offset(key);
            return;
        }
        self.diagnose_array_offset(recv, key);
    }

    /// Whether `arr` holds an element under `key`, by hash lookup on the
    /// NORMALIZED key — which is what `array_key_exists` and `isset` ask.
    ///
    /// `None` for a receiver that is not an array, leaving the caller to handle
    /// an object's properties. The `array_key_exists` builtin used to walk every
    /// pair comparing `to_str` renderings, which is O(n) per call and answers
    /// the wrong question: `array_key_exists(1.7, [1 => "x"])` is true, because
    /// 1.7 narrows to the int key 1, and the string forms `"1.7"` and `"1"`
    /// never match.
    pub fn array_has_key(&mut self, arr: &Value, key: &Value) -> Option<bool> {
        if !matches!(self.as_array(arr), Some(PhpObj::Array { .. })) {
            return None;
        }
        self.diagnose_offset(key);
        let k = self.norm_key(key);
        match self.as_array(arr) {
            Some(PhpObj::Array { entries, .. }) => Some(entries.contains_key(&k)),
            _ => None,
        }
    }

    /// The `<function>(): ` a `php_error_docref` diagnostic raised for the code
    /// running now carries: `f()` for its function, `Unknown` at the top level.
    pub(crate) fn active_function_name(&self) -> String {
        match self.scopes.iter().rev().find(|s| !s.internal) {
            Some(scope) if scope.name.is_some() => {
                format!("{}()", self.trace_frame_name(scope).replace("->", "::"))
            }
            // `php_verror` with no function: the bare word, no parentheses.
            _ => "Unknown".to_string(),
        }
    }

    /// A deprecation raised while the request shuts down: `PHP Request
    /// Shutdown: …` in `Unknown` on line 0.
    pub(crate) fn deprecated_at_shutdown(&mut self, msg: impl std::fmt::Display) {
        self.unknown_site = true;
        self.diagnose(
            "Deprecated",
            errlevel::E_DEPRECATED,
            0,
            format!("PHP Request Shutdown: {msg}"),
        );
        self.unknown_site = false;
    }

    /// Emit a PHP `Notice`.
    pub fn notice(&mut self, msg: impl std::fmt::Display) {
        self.diagnose("Notice", errlevel::E_NOTICE, warn_line(), msg);
    }

    /// The one place a non-fatal diagnostic is rendered, for every severity.
    ///
    /// Two independent gates silence it: `@expr` suppression (lexically scoped,
    /// leaves the level alone) and the `error_reporting` mask (a global the script
    /// or `-d` sets). A diagnostic whose `E_*` bit is clear is not merely hidden —
    /// it is never written, so it cannot land in an output buffer either.
    /// Once past those two gates a diagnostic is written TWICE, to two streams
    /// under two separate ini flags — `display_errors` puts the copy the program
    /// sees on stdout, `log_errors` puts a `PHP `-prefixed copy on stderr. Both
    /// default on, so the ordinary run emits both; `-d log_errors=0` leaves only
    /// the stdout copy and `-d display_errors=0` only the stderr one.
    /// The stderr copy is written FIRST: `php_error_cb` logs before it displays,
    /// which is what an interleaved `2>&1` capture shows.
    pub fn diagnose(&mut self, severity: &str, level: i64, line: u32, msg: impl std::fmt::Display) {
        // Code a PHP-written prelude method runs stands for an internal
        // function, whose diagnostics name the user's call site.
        let line = prelude_call_site_line(self).unwrap_or(line);
        // `set_error_handler`: a level the live handler takes is queued for it
        // instead of displayed — the handler is PHP code, which cannot run
        // inside this borrow, so it runs at the next builtin boundary (see
        // [`drain_error_handlers`]). `@` does not stop it; the handler sees the
        // narrowed `error_reporting()` instead. While a handler runs, its own
        // diagnostics take the default path, as the reference unsets the
        // handler for the duration of the call.
        if !self.in_error_handler && level & errlevel::HANDLEABLE != 0 {
            if let Some((handler, mask)) = self.error_handlers.last() {
                if !matches!(handler, Value::Undef) && mask & level != 0 {
                    let file = self.current_file().to_string();
                    self.queued_errors.push((
                        severity.to_string(),
                        level,
                        msg.to_string(),
                        file,
                        line,
                    ));
                    ERRORS_QUEUED.with(|q| q.set(true));
                    return;
                }
            }
        }
        self.diagnose_default(severity, level, line, msg);
    }

    /// The reference's own error display, with no user handler consulted —
    /// also what a handler that returns `false` falls back to.
    fn diagnose_default(
        &mut self,
        severity: &str,
        level: i64,
        line: u32,
        msg: impl std::fmt::Display,
    ) {
        self.last_error = Some((
            level,
            msg.to_string(),
            self.current_file().to_string(),
            line,
        ));
        if self.suppress > 0 || self.error_reporting & level == 0 {
            return;
        }
        let body = format!("{msg} in {} on line {line}", self.current_file());
        if self.ini_flag("log_errors") {
            Self::write_err(format!("PHP {severity}:  {body}\n").as_bytes());
        }
        if self.ini_flag("display_errors") {
            self.write_out(&format!("\n{severity}: {body}\n"));
        }
    }

    /// Whether an ini setting reads as ON. PHP's ini booleans are stored `"1"` /
    /// `"0"` but a php.ini may spell them `On`/`Off`/`true`/`false`/`yes`/`no`,
    /// and an unset name is off.
    fn ini_flag(&self, name: &str) -> bool {
        match self.ini.get(name).map(|s| s.trim()) {
            None | Some("") | Some("0") => false,
            Some(s) => {
                !(s.eq_ignore_ascii_case("off")
                    || s.eq_ignore_ascii_case("false")
                    || s.eq_ignore_ascii_case("no"))
            }
        }
    }

    /// The current `error_reporting` mask.
    pub fn error_reporting(&self) -> i64 {
        self.error_reporting
    }

    /// What `preg_last_error()` reports.
    pub fn preg_error(&self) -> i64 {
        self.preg_error
    }

    /// Record the outcome of a `preg_*` call that reached the regex compiler.
    /// Called on SUCCESS too, which is what clears a previous failure: in the
    /// reference a pattern that compiles resets the state even when the match
    /// then finds nothing.
    pub fn set_preg_error(&mut self, code: i64) {
        self.preg_error = code;
    }

    /// `strtok`'s saved subject and resume offset, or `None` before the first
    /// two-argument call and after the subject has been exhausted.
    pub fn strtok_state(&self) -> Option<(String, usize)> {
        self.strtok_state.clone()
    }

    /// Install a fresh `strtok` subject (two-argument form) or advance/clear the
    /// resume offset. `None` is the "tokenization finished" state, which is what
    /// makes a later one-argument call answer `false` rather than restart.
    pub fn set_strtok_state(&mut self, state: Option<(String, usize)>) {
        self.strtok_state = state;
    }

    /// `error_get_last()`: an array of the last diagnostic's `type`, `message`,
    /// `file` and `line`, or null when none was raised (or it was cleared).
    pub fn error_get_last(&mut self) -> Value {
        let Some((level, msg, file, line)) = self.last_error.clone() else {
            return Value::Undef;
        };
        let arr = self.new_array();
        for (k, v) in [
            ("type", Value::int(level)),
            ("message", Value::str(msg)),
            ("file", Value::str(file)),
            ("line", Value::int(line as i64)),
        ] {
            self.arr_set_key(&arr, &Value::str(k.to_string()), v);
        }
        arr
    }

    /// `error_clear_last()`.
    pub fn error_clear_last(&mut self) {
        self.last_error = None;
    }

    /// Set the `error_reporting` mask, returning the previous one — what
    /// `error_reporting($level)` reports back. The ini store is updated with the
    /// decimal spelling, so `ini_get('error_reporting')` follows the function.
    pub fn set_error_reporting(&mut self, level: i64) -> i64 {
        self.ini
            .insert("error_reporting".to_string(), level.to_string());
        std::mem::replace(&mut self.error_reporting, level)
    }

    /// Read an ini setting as `ini_get` returns it — always a STRING, even for a
    /// numeric setting. `None` means the name is not a setting this engine knows,
    /// which `ini_get` reports as `false`.
    ///
    /// `error_reporting` is stored here VERBATIM as well as parsed into the mask,
    /// because the two can disagree: `ini_set('error_reporting', '12abc')` leaves
    /// `ini_get` reporting `"12abc"` while the mask becomes 12.
    ///
    /// DIVERGENCE: only the settings `default_ini` lists are known — PHP core
    /// plus `date` and `pcre`. A name belonging to an optional extension the
    /// reference happened to be built with (`mysqli.default_host`), or one whose
    /// default is that build's install prefix (`extension_dir`), reports `false`
    /// rather than a value that would be wrong on another machine.
    pub fn ini_get(&self, name: &str) -> Option<String> {
        self.ini.get(name).cloned()
    }

    /// Write an ini setting, returning the previous value — or `None` for a name
    /// the engine does not know, which `ini_set` reports as `false` without
    /// storing anything (PHP does not let `ini_set` invent settings).
    ///
    /// `error_reporting` also updates the mask, by PHP's ordinary string-to-int
    /// coercion — NOT the constant-expression scanner. `E_ALL & ~E_NOTICE` is a
    /// php.ini spelling, understood on the `-d`/ini path only; handed to
    /// `ini_set` at run time it reads as the integer 0 and mutes everything,
    /// which is exactly what the reference does with it.
    ///
    /// Two more ways a write is refused, both of which report `false` and change
    /// nothing: a setting that is not runtime-changeable (see `INI_FIXED`), and
    /// a value the setting rejects (see `ini_value_rejected`).
    pub fn ini_set(&mut self, name: &str, value: &str) -> Option<String> {
        if INI_FIXED.contains(&name) {
            return None;
        }
        let level = (name == "error_reporting")
            .then(|| self.to_number(&Value::str(value.to_string())).to_int());
        let slot = self.ini.get_mut(name)?;
        let old = std::mem::replace(slot, value.to_string());
        if let Some(level) = level {
            self.error_reporting = level;
        }
        Some(old)
    }

    /// One argument as a stack trace renders it. PHP does not print values in
    /// full here: a string is single-quoted and cut to 15 characters with a
    /// literal `...` inside the quotes, and an array or object collapses to its
    /// kind, so a trace can never be arbitrarily long.
    fn trace_arg(&self, v: &Value) -> String {
        match v {
            Value::Undef => "NULL".to_string(),
            Value::Bool(true) => "true".to_string(),
            Value::Bool(false) => "false".to_string(),
            Value::Str(s) => format!("'{}'", escape_trace_string(s)),
            Value::Obj(_) if self.is_array(v) => "Array".to_string(),
            Value::Obj(_) if self.is_stream(v) => self.to_str(v),
            Value::Obj(_) => match self.instance_class(v) {
                Some(c) => format!("Object({c})"),
                None => "Object(stdClass)".to_string(),
            },
            // A float keeps a fractional part in a trace even when it has none:
            // the reference renders trace arguments with `zero_frac` set, so a
            // whole-valued float reads `1.0` and stays distinguishable from the
            // int `1`. `INF`/`NAN` are left alone.
            Value::Float(f) => {
                let s = self.to_str(v);
                if f.is_finite() && !s.contains('.') && !s.contains('E') {
                    format!("{s}.0")
                } else {
                    s
                }
            }
            other => self.to_str(other),
        }
    }

    /// The `UnhandledMatchError` message for a subject that matched no arm.
    ///
    /// A SCALAR (and `null`) is rendered exactly as a stack trace renders an
    /// argument — `'hi'`, `1.0`, `true`, `NULL` — which is the same
    /// `smart_str_append_scalar` the reference calls for both. Anything else is
    /// `of type <name>`: `array`, `resource`, or the class, since a container
    /// has no rendering short enough for a message.
    pub fn match_error_message(&self, v: &Value) -> String {
        let rendered = match v {
            Value::Undef | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Str(_) => {
                self.trace_arg(v)
            }
            _ if self.is_stream(v) => "of type resource".to_string(),
            _ => format!("of type {}", crate::stdlib::types::debug_type(self, v)),
        };
        format!("Unhandled match case {rendered}")
    }

    /// A frame's name as a trace prints it: `f`, `A->m` for an instance method,
    /// `A::m` for a static one. Frames are recorded as `Class::method` with the
    /// class lowercased (that is the lookup key); the declared spelling comes back
    /// off the `ClassDef`, and the arrow form from whether the frame bound `$this`.
    fn trace_frame_name(&self, scope: &Scope) -> String {
        let Some(name) = &scope.name else {
            return String::new();
        };
        // A closure frame is named for where the LITERAL was written, not for
        // anything about the call: `{closure:K::m():4}`, `{closure:outer():16}`,
        // `{closure:/path/to/script.php:14}` at the top level. The bound-scope
        // prefix (`K->` / `K::`) is still the frame name's own, so the two are
        // assembled here rather than baked into one string — `name` is parsed
        // back for visibility checks and must stay `Scope::{closure}`.
        if let Some(site) = &scope.closure_site {
            let rendered = match site {
                DeclSite::Closure(inner, line) => {
                    format!(
                        "{{closure:{}:{line}}}",
                        inner.render(scope.file.as_deref().unwrap_or(&self.script_name))
                    )
                }
                // A closure body always carries a `Closure` site; anything else
                // is a frame that was handed one it should not have.
                other => other.render(scope.file.as_deref().unwrap_or(&self.script_name)),
            };
            return match name.split_once("::") {
                Some((class, _)) => {
                    let class = self
                        .classes
                        .get(class)
                        .map(|d| d.name.as_str())
                        .unwrap_or(class);
                    let sep = if !matches!(scope.vars.get("this"), Slot::Unset) {
                        "->"
                    } else {
                        "::"
                    };
                    format!("{class}{sep}{rendered}")
                }
                None => rendered,
            };
        }
        let Some((class, method)) = name.split_once("::") else {
            return name.clone();
        };
        let class = self
            .classes
            .get(class)
            .map(|d| d.name.as_str())
            .unwrap_or(class);
        let sep = if !matches!(scope.vars.get("this"), Slot::Unset) {
            "->"
        } else {
            "::"
        };
        format!("{class}{sep}{method}")
    }

    /// PHP's `Stack trace:` body for the call stack as it stands right now —
    /// captured when an exception object is *created*, which is when PHP snapshots
    /// it, because by the time the throw reaches the top the frames are gone.
    ///
    /// Frame `#i` names the function that was entered and the file/line of the
    /// call that entered it, so its call site is the *caller's* current line. The
    /// global scope is not a frame: it closes the list as `#N {main}`.
    ///
    /// A frame entered from inside a library function (an `array_map` callback,
    /// say) has no PHP call site, so its `file(line)` half is the literal
    /// `[internal function]` — the library frame below it carries the real one.
    /// A closure frame is named the way PHP 8.4 names it — see
    /// `trace_frame_name`.
    pub fn backtrace(&self) -> String {
        // The frames outermost first, each with the file and line its code is
        // at: a scope, then any include running on it. A frame's line is where
        // it called deeper — an include's own line for a scope that is running
        // one, the scope's live line otherwise.
        struct Frame<'a> {
            label: Option<String>,
            file: &'a str,
            line: u32,
            internal: bool,
            // A method of a PHP-written prelude class, which stands in for
            // an internal one.
            prelude: bool,
            // A prelude frame that is no method of the class it stands in for: a
            // `__`-named private helper, a helper class, or a closure the
            // prelude wrote. The reference has no frame for any of them.
            helper: bool,
            // A frame the reference has none for at all — see
            // `Scope::transparent` — or a prelude `__call`, which forwards to an
            // inner iterator the way `spl_dual_it_get_method` does.
            transparent: bool,
        }
        let mut frames: Vec<Frame<'_>> = Vec::new();
        for (i, scope) in self.scopes.iter().enumerate() {
            let incs: Vec<&IncludeFrame> = self.includes.iter().filter(|f| f.depth == i).collect();
            let label = (i > 0).then(|| {
                // A STRING key in a frame's argument array is a named argument
                // the callee could not place — the only way one survives to
                // here — and the reference renders it `name: value`.
                let names = self.frame_arg_names(i);
                let args = self
                    .frame_args(i)
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                    .map(|(j, v)| match names.get(j).and_then(Option::as_deref) {
                        Some(n) => format!("{n}: {}", self.trace_arg(v)),
                        None => self.trace_arg(v),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}({args})", self.trace_frame_name(scope))
            });
            frames.push(Frame {
                label,
                file: self.scope_file(i),
                line: incs.first().map_or(scope.line, |f| f.call_line),
                internal: scope.internal,
                prelude: scope.name.as_deref().is_some_and(is_prelude_frame),
                helper: scope.name.as_deref().is_some_and(is_prelude_frame)
                    && (scope.closure_site.is_some()
                        || scope.name.as_deref().is_some_and(is_prelude_helper_name)),
                transparent: scope.transparent
                    || scope
                        .name
                        .as_deref()
                        .is_some_and(|n| is_prelude_frame(n) && n.ends_with("::__call")),
            });
            for (k, inc) in incs.iter().enumerate() {
                frames.push(Frame {
                    label: Some(inc.label.clone()),
                    file: &inc.file,
                    line: incs.get(k + 1).map_or(scope.line, |f| f.call_line),
                    internal: false,
                    prelude: false,
                    helper: false,
                    transparent: false,
                });
            }
        }
        // A prelude method is internal code: what it calls on its own behalf
        // (its helpers, the library functions it uses) is invisible, and a
        // method it calls the way the reference's C calls one — a user
        // `compare()`, an `accept()` — was entered from `[internal function]`.
        // A frame is reported as called from the nearest VISIBLE frame above it.
        let visible: Vec<bool> = (0..frames.len())
            .map(|j| {
                j == 0
                    || !(frames[j].transparent
                        || frames[j - 1].prelude && (frames[j].helper || frames[j].internal))
            })
            .collect();
        let mut out = String::new();
        let mut n = 0;
        for j in (1..frames.len()).rev() {
            if !visible[j] {
                continue;
            }
            let mut k = j - 1;
            while !visible[k] {
                k -= 1;
            }
            let caller = &frames[k];
            let site = if caller.internal || caller.prelude {
                "[internal function]".to_string()
            } else {
                format!("{}({})", caller.file, caller.line)
            };
            let label = frames[j].label.as_deref().unwrap_or_default();
            out.push_str(&format!(
                "#{n} {site}: {label}
"
            ));
            n += 1;
        }
        out.push_str(&format!("#{n} {{main}}"));
        out
    }

    /// Push a call frame for a LIBRARY function so [`PhpHost::backtrace`] names
    /// it, with `args` as the array the trace renders. The counterpart to
    /// [`PhpHost::pop_internal_frame`]; `throw_from_internal` does the same thing
    /// inline for an exception, and this is the form a diagnostic can use, where
    /// no exception object is built.
    pub fn push_internal_frame(&mut self, func: &str, line: u32, args: Value) {
        self.scopes.push(Scope {
            name: Some(func.to_string()),
            line,
            internal: true,
            ..Scope::default()
        });
        self.set_var("@args", args);
    }

    pub fn pop_internal_frame(&mut self) {
        self.scopes.pop();
    }

    /// Frame `idx`'s call arguments as the reference reports them, or `None` if
    /// that frame is not a call frame at all (the global scope has no `@args`).
    ///
    /// A position covered by a declared parameter is reported as that
    /// parameter's CURRENT value, which is why `invoke` stashes the parameter's
    /// NAME there rather than the value passed. Both readers need it and both
    /// must agree: `func_get_args()` answers `[99]` for `function f($a) { $a =
    /// 99; … }` called as `f(1)`, and the stack trace renders that frame as
    /// `f(99)` for the same reason. A frame pushed for a LIBRARY function has
    /// `@args` and no `@argnames`, so every position there falls through to the
    /// value.
    /// The NAME each of frame `idx`'s arguments was passed under, positionally
    /// aligned with [`frame_args`]. `None` for an ordinary positional argument,
    /// which is all of them except a name a variadic library function could not
    /// place — see `throw_from_internal_named`.
    ///
    /// [`frame_args`]: PhpHost::frame_args
    pub(crate) fn frame_arg_names(&self, idx: usize) -> Vec<Option<String>> {
        let args = self.get_var_in(idx, "@args");
        if !self.is_array(&args) {
            return Vec::new();
        }
        self.array_pairs(&args)
            .unwrap_or_default()
            .iter()
            .map(|(k, _)| match k {
                Value::Str(s) => Some(s.to_string()),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn frame_args(&self, idx: usize) -> Option<Vec<Value>> {
        let args = self.get_var_in(idx, "@args");
        if !self.is_array(&args) {
            return None;
        }
        let names = self.get_var_in(idx, "@argnames");
        Some(
            self.array_pairs(&args)
                .unwrap_or_default()
                .iter()
                .enumerate()
                .map(
                    |(pos, (_, v))| match self.index_get(&names, &Value::int(pos as i64)) {
                        Value::Undef => v.clone(),
                        n => self.get_var_in(idx, &self.to_str(&n)),
                    },
                )
                .collect(),
        )
    }

    /// [`PhpHost::frame_args`] for the frame that is executing — what the
    /// `func_get_args` family reports on.
    pub(crate) fn current_frame_args(&self) -> Option<Vec<Value>> {
        self.frame_args(self.scopes.len().checked_sub(1)?)
    }

    /// Read a variable out of a specific frame — the backtrace needs each frame's
    /// own hidden `@args`, not the innermost one's.
    fn get_var_in(&self, idx: usize, name: &str) -> Value {
        self.scopes
            .get(idx)
            .map(|s| s.vars.get(name).clone())
            .map(|s| self.read_slot(&s))
            .unwrap_or(Value::Undef)
    }

    /// Report a fatal error the way the PHP CLI does: the *display* copy goes on
    /// the output stream (so it lands inside an open `ob_start` buffer and
    /// interleaves with the program's own output exactly as the reference does),
    /// and `log_errors` puts a `PHP `-prefixed copy on stderr.
    /// Both copies are gated the way [`PhpHost::diagnose`]'s are, and by the same
    /// three gates — a fatal is not exempt from any of them. `error_reporting`
    /// masks it by its own level (`E_ERROR` for a fatal, `E_PARSE` for a syntax
    /// error), so `php -d error_reporting=0` runs an uncaught exception to
    /// completion in silence; `-d display_errors=0` leaves it on stderr alone
    /// and `-d log_errors=0` on stdout alone. `fatal_reported` is set through
    /// every one of them, because the run failed whether or not anyone was told
    /// and the process still has to exit 255.
    pub fn fatal(&mut self, severity: &str, body: &str) {
        let level = if severity == "Parse error" {
            errlevel::E_PARSE
        } else {
            errlevel::E_ERROR
        };
        if self.error_reporting & level == 0 {
            self.fatal_reported = true;
            return;
        }
        if self.ini_flag("log_errors") {
            Self::write_err(format!("PHP {severity}:  {body}\n").as_bytes());
        }
        if self.ini_flag("display_errors") {
            self.write_out(&format!("\n{severity}: {body}\n"));
        }
        self.fatal_reported = true;
    }

    /// Whether a fatal has already been displayed in PHP's shape, so the CLI
    /// wrapper must not print it again.
    pub fn fatal_reported(&self) -> bool {
        self.fatal_reported
    }

    /// The type name PHP uses inside a diagnostic — the short spelling (`int`,
    /// `bool`), not `gettype`'s (`integer`, `boolean`).
    pub fn diag_type(&self, v: &Value) -> &'static str {
        match v {
            Value::Undef => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "string",
            Value::Obj(_) => {
                if self.is_array(v) {
                    "array"
                } else {
                    "object"
                }
            }
            _ => "null",
        }
    }

    /// The type name `Trying to access array offset on …` uses. It differs from
    /// [`diag_type`] for booleans, which it spells as the literal `true`/`false`.
    fn diag_offset_type(&self, v: &Value) -> &'static str {
        match v {
            Value::Bool(true) => "true",
            Value::Bool(false) => "false",
            other => self.diag_type(other),
        }
    }

    /// An array key as a diagnostic renders it: a string key is quoted, an
    /// integer key is bare (`Undefined array key "k"` vs `Undefined array key 7`).
    pub fn diag_key(&self, key: &Value) -> String {
        match self.norm_key(key) {
            ArrayKey::Int(n) => n.to_string(),
            ArrayKey::Str(s) => format!("\"{s}\""),
        }
    }

    pub fn set_error(&mut self, msg: impl Into<String>) {
        if self.error.is_none() {
            self.error = Some(msg.into());
        }
    }

    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    // ── variables ──────────────────────────────────────────────────────────

    /// The index of the scope a variable name resolves in: the global frame for a
    /// superglobal, else the innermost frame.
    fn scope_idx(&self, name: &str) -> usize {
        if is_superglobal(name) {
            0
        } else {
            self.scopes.len().saturating_sub(1)
        }
    }

    pub fn get_var(&self, name: &str) -> Value {
        let idx = self.scope_idx(name);
        let Some(scope) = self.scopes.get(idx) else {
            return Value::Undef;
        };
        self.read_slot(scope.vars.get(name))
    }

    /// The value a slot holds: a reference reads through its shared cell, an
    /// unset slot reads as PHP `null`.
    fn read_slot(&self, s: &Slot) -> Value {
        match s {
            Slot::Val(v) => v.clone(),
            Slot::Ref(c) => self.ref_cells.get(*c).cloned().unwrap_or(Value::Undef),
            Slot::Unset => Value::Undef,
        }
    }

    /// Is `name` BOUND in the scope it resolves in?
    ///
    /// Distinct from a non-`Undef` `get_var`, and the difference is the whole
    /// point: PHP `null` is `Value::Undef` here (fusevm has no null), so an unset
    /// name and a name holding `null` read back identically. Only the binding
    /// itself separates them, which is what `compact()` and `isset()`-shaped
    /// checks need — `$a = null; compact('a')` yields `['a' => null]` with no
    /// diagnostic, while an unset `$a` yields `[]` and a warning.
    pub fn var_defined(&self, name: &str) -> bool {
        let idx = self.scope_idx(name);
        let Some(scope) = self.scopes.get(idx) else {
            return false;
        };
        !matches!(scope.vars.get(name), Slot::Unset)
    }

    /// Whether `name` has a slot in the current frame, bound or not: a compiled
    /// variable the frame mentions anywhere (the reference's CV, present in the
    /// symbol table as an `IS_UNDEF` indirect until first written), or one bound
    /// dynamically and since unset.
    pub fn var_has_slot(&self, name: &str) -> bool {
        let idx = self.scope_idx(name);
        self.scopes
            .get(idx)
            .is_some_and(|s| s.vars.slot_of(name).is_some())
    }

    pub fn set_var(&mut self, name: &str, val: Value) {
        let idx = self.scope_idx(name);
        let Some(scope) = self.scopes.get_mut(idx) else {
            return;
        };
        let i = scope.vars.ensure_slot(name);
        self.write_slot(idx, i, val);
    }

    /// Store into a slot: a reference writes through its shared cell so every
    /// alias sees it, anything else takes the value directly.
    fn write_slot(&mut self, scope_idx: usize, i: u32, val: Value) {
        let cell = match self.scopes.get(scope_idx).map(|s| s.vars.at(i)) {
            Some(Slot::Ref(c)) => Some(*c),
            _ => None,
        };
        match cell {
            Some(c) => {
                if let Some(slot) = self.ref_cells.get_mut(c) {
                    *slot = val;
                }
            }
            None => {
                if let Some(scope) = self.scopes.get_mut(scope_idx) {
                    scope.vars.put(i, Slot::Val(val));
                }
            }
        }
    }

    /// The current frame's bound variables, in the order they were first
    /// bound — what `get_defined_vars()` answers.
    ///
    /// Slot order IS binding order: the compiler numbers a scope's parameters
    /// first and then each local at its first mention, and a name reserved
    /// later only ever appends. Iterating the name index instead would answer
    /// in hash order, which PHP's output is not.
    ///
    /// An unset name is absent while a name bound to `null` is present — the
    /// distinction `Slot::Unset` exists for. `$this` and the compiler's own
    /// `@`-prefixed entries are not the user's variables and are left out.
    pub fn defined_vars(&self) -> Vec<(String, Value)> {
        let Some(scope) = self.scopes.last() else {
            return Vec::new();
        };
        let mut by_slot: Vec<(u32, &String)> = scope
            .vars
            .index
            .iter()
            .map(|(name, &i)| (i, name))
            .collect();
        by_slot.sort_unstable_by_key(|(i, _)| *i);
        by_slot
            .into_iter()
            // `$GLOBALS` is a view of the global frame rather than a variable
            // in it, and PHP never lists it here.
            .filter(|(_, name)| {
                !name.starts_with('@') && !matches!(name.as_str(), "this" | "GLOBALS")
            })
            .filter_map(|(i, name)| match scope.vars.at(i) {
                Slot::Unset => None,
                slot => Some((name.clone(), self.read_slot(slot))),
            })
            .collect()
    }

    /// Read the current frame's slot `i`. The compiler resolved the name to this
    /// index, so there is no name to test against the superglobals or to hash.
    pub fn slot_get(&mut self, i: u32) -> Value {
        let s = match self.scopes.last() {
            Some(sc) => sc.vars.at(i).clone(),
            None => Slot::Unset,
        };
        if matches!(s, Slot::Unset) {
            // Only the diagnostic needs the name back, and only on the path that
            // is already an error, so the reverse lookup is not on the hot path.
            if let Some(n) = self.slot_name(i) {
                if !n.starts_with('@') {
                    self.warn(format_args!("Undefined variable ${n}"));
                }
            }
            return Value::Undef;
        }
        self.read_slot(&s)
    }

    /// Read slot `i` with no `Undefined variable` diagnostic — the isset/`empty`
    /// context, where an unbound name is a legitimate answer rather than a bug.
    pub fn slot_get_quiet(&self, i: u32) -> Value {
        match self.scopes.last() {
            Some(sc) => self.read_slot(sc.vars.at(i)),
            None => Value::Undef,
        }
    }

    /// Write the current frame's slot `i`, through the shared cell when the slot
    /// holds a reference binding.
    pub fn slot_set(&mut self, i: u32, val: Value) {
        let idx = self.scopes.len().saturating_sub(1);
        self.write_slot(idx, i, val);
    }

    /// The name a slot was resolved from, for a diagnostic. Linear in the
    /// frame's variable count and only reached when reporting an unbound read.
    fn slot_name(&self, i: u32) -> Option<String> {
        self.scopes
            .last()?
            .vars
            .index
            .iter()
            .find(|(_, &v)| v == i)
            .map(|(k, _)| k.clone())
    }

    /// Reserve the GLOBAL frame's slots in the order the main chunk numbered
    /// them. Separate from `seed_slots`, which seeds the innermost
    /// frame: the main chunk runs in the global frame whatever is on the stack.
    pub fn seed_global_slots(&mut self, names: &[String]) {
        if let Some(scope) = self.scopes.first_mut() {
            scope.vars.renumber(names);
        }
    }

    /// Reserve this frame's slots in the order the compiler numbered them, so
    /// index `n` in the chunk and index `n` in the frame are the same variable.
    /// Called once per call, before any binding.
    fn seed_slots(&mut self, names: &[String]) {
        if let Some(scope) = self.scopes.last_mut() {
            for n in names {
                scope.vars.ensure_slot(n);
            }
        }
    }

    /// `$target = &$source` — bind `target` as a reference to `source`, so both
    /// names share one storage cell (either's mutation is visible to the other).
    /// The reference cell `name` resolves to, promoting a plain variable into
    /// one first, wrapped in a [`PhpObj::Ref`] handle. What `use (&$v)` captures
    /// — see [`PhpHost::bind_ref_slot`] for the other half.
    pub fn ref_cell_of(&mut self, name: &str) -> Value {
        let slot = self.ref_slot_of(name);
        self.objs.push(PhpObj::Ref { slot });
        Value::Obj((self.objs.len() - 1) as u32)
    }

    /// Point `name` in the current scope at an existing reference cell, so it
    /// and whatever else shares the cell are one variable.
    pub fn bind_ref_slot(&mut self, name: &str, slot: usize) {
        let idx = self.scope_idx(name);
        if let Some(scope) = self.scopes.get_mut(idx) {
            let i = scope.vars.ensure_slot(name);
            scope.vars.put(i, Slot::Ref(slot));
        }
    }

    /// Allocate a reference cell that no variable owns yet, holding `val`, and
    /// return a [`PhpObj::Ref`] handle to it together with its slot.
    ///
    /// Builtins that hand an element to a user callback by reference
    /// (`array_walk`) use this: pass the handle as the argument, let the callee's
    /// by-ref parameter bind the slot, then read the slot back with
    /// [`PhpHost::ref_cell_value`] to see what the callback wrote.
    pub fn new_ref_cell(&mut self, val: Value) -> (Value, usize) {
        self.ref_cells.push(val);
        let slot = self.ref_cells.len() - 1;
        self.objs.push(PhpObj::Ref { slot });
        (Value::Obj((self.objs.len() - 1) as u32), slot)
    }

    /// Read the current contents of the reference cell at `slot`.
    pub fn ref_cell_value(&self, slot: usize) -> Value {
        self.ref_cells.get(slot).cloned().unwrap_or(Value::Undef)
    }

    /// The reference slot a value denotes, if it is a [`PhpObj::Ref`] handle.
    ///
    /// A stored `Ref` handle is how an array element or an object property that a
    /// `&` binding has made into a reference is represented: the container keeps
    /// the handle, every read derefs it, and every write goes through the cell, so
    /// the element and the alias are one storage location — PHP's `IS_REFERENCE`
    /// zval in the slot.
    pub fn ref_slot_of_value(&self, v: &Value) -> Option<usize> {
        match v {
            Value::Obj(h) => match self.objs.get(*h as usize) {
                Some(PhpObj::Ref { slot }) => Some(*slot),
                _ => None,
            },
            _ => None,
        }
    }

    /// Resolve a stored value: a `Ref` handle reads as its cell's contents,
    /// everything else as itself. Applied on the way out of every container read
    /// so a reference in a slot is invisible to code that only reads the value.
    fn deref(&self, v: Value) -> Value {
        match self.ref_slot_of_value(&v) {
            Some(slot) => self.ref_cells.get(slot).cloned().unwrap_or(Value::Undef),
            None => v,
        }
    }

    /// Write `val` into the reference cell at `slot`.
    fn ref_cell_set(&mut self, slot: usize, val: Value) {
        if let Some(cell) = self.ref_cells.get_mut(slot) {
            *cell = val;
        }
    }

    /// The reference cell `name` resolves to, creating one — and moving the
    /// variable's current value into it — if the variable is still a plain one.
    fn ref_slot_of(&mut self, name: &str) -> usize {
        let idx = self.scope_idx(name);
        if let Some(Slot::Ref(c)) = self.scopes.get(idx).map(|s| s.vars.get(name)) {
            return *c;
        }
        let cur = self
            .scopes
            .get(idx)
            .map(|s| s.vars.get(name).clone())
            .map(|s| self.read_slot(&s))
            .unwrap_or(Value::Undef);
        self.ref_cells.push(cur);
        let slot = self.ref_cells.len() - 1;
        if let Some(scope) = self.scopes.get_mut(idx) {
            let i = scope.vars.ensure_slot(name);
            scope.vars.put(i, Slot::Ref(slot));
        }
        slot
    }

    /// The reference cell for the array element `$name[k1]..[kN]`, promoting the
    /// element into a reference (and auto-vivifying the path, as PHP does for a
    /// `&` lvalue) if it is not one already. `keys` must be non-empty.
    ///
    /// This is the storage half of `$r = &$a[k]`: the element slot keeps a
    /// [`PhpObj::Ref`] handle to the returned cell, so a later write through
    /// either name lands in the one cell.
    pub fn elem_ref_slot(&mut self, name: &str, keys: &[Value]) -> usize {
        let Some((last, inter)) = keys.split_last() else {
            return self.ref_slot_of(name);
        };
        let arr = self.ensure_path_array(name, inter);
        self.arr_elem_ref_slot(&arr, last)
    }

    /// The reference cell for `arr[key]` (a handle), promoting the element into a
    /// reference if it is not one already.
    pub fn arr_elem_ref_slot(&mut self, arr: &Value, key: &Value) -> usize {
        let k = self.norm_key(key);
        if let Some(slot) = self.entry_ref_slot(arr, &k) {
            return slot;
        }
        // PHP vivifies a `&`-taken element to null when it does not exist yet.
        let cur = match self.as_array(arr) {
            Some(PhpObj::Array { entries, .. }) => entries.get(&k).cloned().unwrap_or(Value::Undef),
            _ => Value::Undef,
        };
        self.ref_cells.push(cur);
        let slot = self.ref_cells.len() - 1;
        self.objs.push(PhpObj::Ref { slot });
        let handle = Value::Obj((self.objs.len() - 1) as u32);
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(arr)
        {
            if let ArrayKey::Int(n) = k {
                if n >= *next_index {
                    *next_index = n.saturating_add(1);
                }
            }
            entries.insert(k, handle);
        }
        slot
    }

    /// The reference cell for `$obj->name`, promoting the property into a
    /// reference if it is not one already.
    pub fn prop_ref_slot_ensure(&mut self, recv: &Value, name: &str) -> usize {
        if let Some(slot) = self.prop_ref_slot(recv, name) {
            return slot;
        }
        let cur = match self.as_array(recv) {
            Some(PhpObj::Object { props, .. }) => props.get(name).cloned().unwrap_or(Value::Undef),
            _ => Value::Undef,
        };
        self.ref_cells.push(cur);
        let slot = self.ref_cells.len() - 1;
        self.objs.push(PhpObj::Ref { slot });
        let handle = Value::Obj((self.objs.len() - 1) as u32);
        self.obj_put_prop(recv, name, handle);
        slot
    }

    /// Store a [`PhpObj::Ref`] handle for `slot` into `$name[k1]..[kN]` — the
    /// `$a[k] = &$x` direction, where the *container slot* becomes the alias.
    /// An empty `keys` binds the plain variable instead.
    pub fn bind_elem_to_slot(&mut self, name: &str, keys: &[Value], slot: usize) {
        let Some((last, inter)) = keys.split_last() else {
            self.bind_ref_slot(name, slot);
            return;
        };
        let arr = self.ensure_path_array(name, inter);
        self.objs.push(PhpObj::Ref { slot });
        let handle = Value::Obj((self.objs.len() - 1) as u32);
        let k = self.norm_key(last);
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(&arr)
        {
            if let ArrayKey::Int(n) = k {
                if n >= *next_index {
                    *next_index = n.saturating_add(1);
                }
            }
            entries.insert(k, handle);
        }
    }

    /// `$name[k1]..[kM][] = &$x` — append a [`PhpObj::Ref`] handle for `slot`.
    ///
    /// Refuses with [`NEXT_ELEMENT_OCCUPIED`] on a saturated array, exactly as
    /// the by-value append does — binding a reference is still an append.
    pub fn append_elem_to_slot(
        &mut self,
        name: &str,
        keys: &[Value],
        slot: usize,
    ) -> Result<(), String> {
        let arr = self.ensure_path_array(name, keys);
        if self.append_slot_taken(&arr) {
            return Err(NEXT_ELEMENT_OCCUPIED.to_string());
        }
        self.objs.push(PhpObj::Ref { slot });
        let handle = Value::Obj((self.objs.len() - 1) as u32);
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(&arr)
        {
            let k = ArrayKey::Int(*next_index);
            *next_index = next_index.saturating_add(1);
            entries.insert(k, handle);
        }
        Ok(())
    }

    /// `$obj->p = &$x` — store a [`PhpObj::Ref`] handle for `slot` in a property.
    pub fn bind_prop_to_slot(&mut self, recv: &Value, name: &str, slot: usize) {
        self.objs.push(PhpObj::Ref { slot });
        let handle = Value::Obj((self.objs.len() - 1) as u32);
        self.obj_put_prop(recv, name, handle);
    }

    /// The running frame's late-static-binding class, or `fallback` (the
    /// enclosing class the compiler baked in) outside any method call.
    pub fn lsb_class(&self, fallback: &str) -> String {
        self.scopes
            .last()
            .and_then(|s| s.static_class.clone())
            .unwrap_or_else(|| fallback.to_string())
    }

    /// Mark the next call as forwarding: it inherits this frame's
    /// late-static-binding class instead of taking the class it names.
    pub fn lsb_forward(&mut self) {
        self.pending_lsb = self.scopes.last().and_then(|s| s.static_class.clone());
    }

    /// Set the late-static-binding class the next call's frame takes, unless a
    /// forwarding call has already claimed it.
    pub fn lsb_set_for_next_call(&mut self, class: &str) {
        if self.pending_lsb.is_none() {
            self.pending_lsb = Some(class.to_string());
        }
    }

    /// Take the late-static-binding class a pushed frame should carry.
    fn lsb_take(&mut self) -> Option<String> {
        self.pending_lsb.take()
    }

    /// Hand the next pushed frame the site of the closure literal being entered.
    pub fn closure_site_for_next_call(&mut self, site: Option<DeclSite>) {
        self.pending_closure_site = site;
    }

    /// Take it. A non-closure call leaves it `None`, which is what a named
    /// function's frame wants.
    fn closure_site_take(&mut self) -> Option<DeclSite> {
        self.pending_closure_site.take()
    }

    /// Publish `slot` as the reference the running `function &f()` returns.
    pub fn set_ret_ref_slot(&mut self, slot: usize) {
        self.ret_ref_slot = Some(slot);
    }

    /// Take the reference cell the last call returned. A call that did not return
    /// by reference has none, so `fallback` is parked in a fresh detached cell —
    /// `$r = &f()` on a by-value function then aliases only its own copy.
    pub fn take_ret_ref_slot(&mut self, fallback: Value) -> usize {
        match self.ret_ref_slot.take() {
            Some(slot) => slot,
            None => {
                self.ref_cells.push(fallback);
                self.ref_cells.len() - 1
            }
        }
    }

    /// `global $name;` — make the running frame's `name` an alias of the GLOBAL
    /// variable of that name.
    ///
    /// The reference defines this as `$name = &$GLOBALS['name']`, and it is a
    /// real reference on both ends: the global is promoted to a shared cell if
    /// it was a plain value, and the local is bound to that same cell. So a
    /// write through either side is seen by the other, and a global that did not
    /// exist yet is CREATED by the binding rather than read as null once.
    ///
    /// At global scope the two frames are the same frame, and the reference
    /// treats `global $x` there as a no-op rather than aliasing a name to
    /// itself.
    pub fn bind_global(&mut self, name: &str) {
        const GLOBAL_FRAME: usize = 0;
        let cur = self.scopes.len().saturating_sub(1);
        if cur == GLOBAL_FRAME {
            return;
        }
        let slot = match self.scopes.get(GLOBAL_FRAME).map(|s| s.vars.get(name)) {
            Some(Slot::Ref(c)) => *c,
            _ => {
                // Promote the global into a cell, carrying whatever it already
                // held so `global $x` does not clear an existing value.
                let cur_val = self
                    .scopes
                    .get(GLOBAL_FRAME)
                    .map(|s| s.vars.get(name).clone())
                    .map(|s| self.read_slot(&s))
                    .unwrap_or(Value::Undef);
                self.ref_cells.push(cur_val);
                let slot = self.ref_cells.len() - 1;
                if let Some(scope) = self.scopes.get_mut(GLOBAL_FRAME) {
                    let i = scope.vars.ensure_slot(name);
                    scope.vars.put(i, Slot::Ref(slot));
                }
                slot
            }
        };
        if let Some(scope) = self.scopes.get_mut(cur) {
            let i = scope.vars.ensure_slot(name);
            scope.vars.put(i, Slot::Ref(slot));
        }
    }

    pub fn ref_bind(&mut self, target: &str, source: &str) {
        let idx = self.scope_idx(source);
        // Resolve the source's cell, promoting a plain variable into one.
        let slot = self.ref_slot_of(source);
        let _ = idx;
        let tidx = self.scope_idx(target);
        if let Some(scope) = self.scopes.get_mut(tidx) {
            let i = scope.vars.ensure_slot(target);
            scope.vars.put(i, Slot::Ref(slot));
        }
    }

    /// `unset($name)` — remove the scope variable (and break any reference
    /// binding for that name, leaving the shared cell intact for other aliases).
    pub fn unset_var(&mut self, name: &str) {
        let idx = self.scope_idx(name);
        if let Some(scope) = self.scopes.get_mut(idx) {
            // The slot stays reserved so an index the compiler handed out is
            // still valid; only the binding goes away.
            if let Some(i) = scope.vars.slot_of(name) {
                scope.vars.put(i, Slot::Unset);
            }
        }
    }

    /// `unset($name[k1]..[kN])` — remove the deepest array element along the key
    /// path. The key is removed without renumbering the remaining keys (PHP
    /// leaves a hole), matching `unset` on an array element.
    pub fn unset_path(&mut self, name: &str, keys: &[Value]) {
        let Some((last, inter)) = keys.split_last() else {
            return;
        };
        // Navigate (read-only, no vivification) to the array holding `last`.
        let mut arr = self.get_var(name);
        for k in inter {
            arr = self.index_get(&arr, k);
        }
        self.diagnose_array_offset(&arr, last);
        let nk = self.norm_key(last);
        if let Some(PhpObj::Array { entries, .. }) = self.as_array_mut(&arr) {
            entries.shift_remove(&nk);
        }
    }

    // ── DAP debug introspection (used only under `--dap`) ────────────────────

    /// Number of active scopes (the debugger's step-depth reference).
    pub fn frame_depth(&self) -> usize {
        self.scopes.len()
    }

    /// Record the source line the innermost frame is executing.
    ///
    /// Two callers, one meaning. Under `--dap` the per-statement line hook drives
    /// it; on every path the ops that can enter a frame or raise a throw record
    /// the op's own line, which is what a backtrace reads back as the call site.
    /// Distinct from [`crate::host::set_warn_line`], which is the line of
    /// the op that is warning *now* and need not survive a nested call returning.
    pub fn set_cur_line(&mut self, line: u32) {
        if let Some(s) = self.scopes.last_mut() {
            s.line = line;
        }
    }

    /// The call stack as `(frame name, line)` pairs, innermost first — for the
    /// DAP `stackTrace`. The global scope is reported as `{main}`.
    pub fn dbg_stack(&self) -> Vec<(String, u32)> {
        self.scopes
            .iter()
            .rev()
            .map(|s| {
                let name = s.name.clone().unwrap_or_else(|| "{main}".to_string());
                (name, s.line)
            })
            .collect()
    }

    /// The innermost frame's locals as `(name, string-cast)` pairs — for DAP
    /// `variables`. Compiler temporaries (`@`-prefixed) are hidden, matching a
    /// debugger's default locals view.
    pub fn dbg_locals(&self) -> Vec<(String, String)> {
        let pairs: Vec<(String, Slot)> = self
            .scopes
            .last()
            .map(|s| {
                s.vars
                    .iter()
                    .filter(|(k, _)| !k.starts_with('@'))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();
        // A reference-bound name reads through its shared cell like any other.
        let vars: Vec<(String, Value)> = pairs
            .into_iter()
            .map(|(k, s)| {
                let v = self.read_slot(&s);
                (k, v)
            })
            .collect();
        vars.into_iter()
            .map(|(n, v)| (format!("${n}"), self.to_str(&v)))
            .collect()
    }

    // ── arrays ─────────────────────────────────────────────────────────────

    /// A fresh `stdClass` holding `props` in the given order, invisible to the
    /// object counter. `json_encode` stages the public-only view of an object
    /// here rather than mutating the real one; because PHP never allocated it,
    /// it must not shift the `#n` handles `var_dump` prints.
    pub fn new_transient_object(&mut self, props: Vec<(String, Value)>) -> Value {
        let v = self.new_object_bare("stdClass", props);
        if let Value::Obj(id) = v {
            self.transient_objs.insert(id);
        }
        v
    }

    /// An instance of `class` carrying exactly `props`, with NO constructor and NO
    /// property defaults. `unserialize` needs this: the reference restores the
    /// recorded state verbatim and never runs `__construct`.
    pub fn new_object_bare(&mut self, class: &str, props: Vec<(String, Value)>) -> Value {
        self.objs.push(PhpObj::Object {
            class: class.to_string(),
            props: props.into_iter().collect(),
        });
        Value::Obj((self.objs.len() - 1) as u32)
    }

    pub fn new_array(&mut self) -> Value {
        self.objs.push(PhpObj::Array {
            entries: IndexMap::new(),
            next_index: 0,
        });
        Value::Obj((self.objs.len() - 1) as u32)
    }

    /// A PHP array is a **value**: assigning one, passing one to a function or
    /// returning one hands over a copy, so a write through the new name is
    /// invisible through the old one. Everything else — an object, a closure,
    /// a generator, a stream — is a handle and is passed through untouched,
    /// which is exactly PHP's own split.
    ///
    /// The copy is deep in the arrays and shallow everywhere else: an array
    /// nested in an array is itself a value and is copied with it, while an
    /// object stored in one is a handle and stays shared. PHP defers the same
    /// copy until a write (copy-on-write), which no program can observe — the
    /// difference is the cost of a copy that turns out to be unread, not the
    /// answer.
    pub fn copy_on_assign(&mut self, v: Value) -> Value {
        let Value::Obj(id) = v else { return v };
        let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.objs.get(id as usize)
        else {
            return v;
        };
        let next_index = *next_index;
        let pairs: Vec<(ArrayKey, Value)> = entries
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let copied: IndexMap<ArrayKey, Value> = pairs
            .into_iter()
            .map(|(k, v)| (k, self.copy_on_assign(v)))
            .collect();
        self.objs.push(PhpObj::Array {
            entries: copied,
            next_index,
        });
        Value::Obj((self.objs.len() - 1) as u32)
    }

    /// Duplicate the object `v` refers to, giving a NEW handle over the same
    /// state — the storage half of `clone $o`, without the `__clone` hook.
    ///
    /// Each property goes through [`PhpHost::copy_on_assign`], which is exactly
    /// PHP's rule for what a clone shares: an array property is a value and is
    /// copied, an object property is a handle and stays shared with the
    /// original. A closure is duplicated whole (its captures were already
    /// copied when it was built).
    ///
    /// `None` for anything that is not a clonable object — an array, a
    /// generator, or a scalar — so the caller can raise the right error, which
    /// differs between the three.
    pub fn clone_obj(&mut self, v: &Value) -> Option<Value> {
        let Value::Obj(id) = v else { return None };
        match self.objs.get(*id as usize)? {
            PhpObj::Object { class, props } => {
                let class = class.clone();
                let pairs: Vec<(String, Value)> =
                    props.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                let props: IndexMap<String, Value> = pairs
                    .into_iter()
                    .map(|(k, v)| (k, self.copy_on_assign(v)))
                    .collect();
                self.objs.push(PhpObj::Object { class, props });
                let copy = (self.objs.len() - 1) as u32;
                // The copy inherits which readonly properties are already
                // written: it carries their values, so they are initialized.
                if let Some(init) = self.readonly_init.get(id).cloned() {
                    self.readonly_init.insert(copy, init);
                }
                Some(Value::Obj(copy))
            }
            dup @ PhpObj::Closure { .. } => {
                let dup = dup.clone();
                self.objs.push(dup);
                Some(Value::Obj((self.objs.len() - 1) as u32))
            }
            _ => None,
        }
    }

    // ── closures ───────────────────────────────────────────────────────────

    /// Build a closure object from a compiler-registered function definition
    /// (`def_name`, a synthetic name in the function table) plus the values
    /// captured at creation time. Returns the new handle, or `Undef` if the
    /// definition is missing (should never happen for compiler-emitted names).
    pub fn make_closure(&mut self, def_name: &str, mut captured: Vec<(String, Value)>) -> Value {
        let Some(def) = self.functions.get(def_name).cloned() else {
            return Value::Undef;
        };
        // `static function (…)` arrives with the marker capture the compiler
        // added. It is not a variable, so it is taken back out before the rest
        // become the closure's bindings.
        let is_static = {
            let n = captured.len();
            captured.retain(|(k, _)| k != STATIC_CLOSURE_CAPTURE);
            captured.len() != n
        };
        // A closure defined inside a method auto-binds the current `$this` and its
        // class scope (PHP semantics), so `$this` and private member access work
        // without an explicit `bindTo`. `None`/`None` at the top level or in a free
        // function keeps the previous behavior exactly.
        let bound_this = match self.get_var("this") {
            // A static closure is never bound to the instance it was written
            // inside, so `$this` is unset in its body and `Closure::bind`
            // refuses to supply one later.
            _ if is_static => None,
            t @ Value::Obj(_) => Some(t),
            _ => None,
        };
        let scope = self.current_class_ctx();
        // `ZEND_DECLARE_LAMBDA_FUNCTION`: the class of the enclosing `$this`
        // when there is one — even for a `static` closure, which drops only the
        // instance — otherwise the enclosing frame's own called scope.
        let called_scope = match self.get_var("this") {
            t @ Value::Obj(_) => self.object_class(&t),
            _ => self.scopes.last().and_then(|s| s.static_class.clone()),
        };
        self.objs.push(PhpObj::Closure {
            params: def.params,
            chunk: Box::new(def.chunk),
            captured,
            bound_this,
            scope,
            called_scope,
            is_generator: def.is_generator,
            ret: def.ret,
            is_static,
            site: def.closure_site,
        });
        Value::Obj((self.objs.len() - 1) as u32)
    }

    /// The closure held by `v` (a handle), cloned for a call: its parameters,
    /// body chunk, captured bindings, and any bound `$this`/scope. `None` if `v`
    /// is not a closure.
    fn closure_of(&self, v: &Value) -> Option<ClosureCall> {
        match self.as_array(v) {
            Some(PhpObj::Closure {
                params,
                chunk,
                captured,
                bound_this,
                scope,
                called_scope,
                is_generator,
                ret,
                site,
                ..
            }) => Some(ClosureCall {
                params: params.clone(),
                chunk: (**chunk).clone(),
                captured: captured.clone(),
                bound_this: bound_this.clone(),
                scope: scope.clone(),
                called_scope: called_scope.clone(),
                is_generator: *is_generator,
                ret: ret.clone(),
                site: site.clone(),
            }),
            _ => None,
        }
    }

    /// A new closure identical to `v` but with `$this` rebound to `this` and the
    /// private-access scope set to `scope` (both optional). Backs
    /// `Closure::bind`/`bindTo`/`call`.
    pub fn rebind_closure(
        &mut self,
        v: &Value,
        this: Option<Value>,
        scope: Option<String>,
    ) -> Option<Value> {
        let (params, chunk, captured, is_generator, ret, is_static, site) = match self.as_array(v) {
            Some(PhpObj::Closure {
                params,
                chunk,
                captured,
                is_generator,
                ret,
                is_static,
                site,
                ..
            }) => (
                params.clone(),
                chunk.clone(),
                captured.clone(),
                *is_generator,
                ret.clone(),
                *is_static,
                site.clone(),
            ),
            _ => return None,
        };
        // `do_closure_bind`: the new object's class, or the new scope without
        // one — in its declared spelling, since a scope may arrive lowercased.
        let called_scope = match &this {
            Some(t) => self.object_class(t),
            None => scope.as_ref().map(|s| {
                self.classes
                    .get(&s.to_ascii_lowercase())
                    .map_or_else(|| s.clone(), |d| d.name.clone())
            }),
        };
        self.objs.push(PhpObj::Closure {
            params,
            chunk,
            captured,
            bound_this: this,
            scope,
            called_scope,
            is_generator,
            ret,
            is_static,
            // The rebound closure is the same literal, so it keeps the site the
            // original was written at — which is what PHP reports.
            site,
        });
        Some(Value::Obj((self.objs.len() - 1) as u32))
    }

    /// Whether `v` is a closure handle.
    /// `zend_get_callable_name_ex` for a closure: the `{closure:<where>:<line>}`
    /// name PHP 8.4 gives the literal, as `ob_list_handlers()` reports it.
    pub fn closure_callable_name(&self, v: &Value) -> Option<String> {
        let cc = self.closure_of(v)?;
        Some(
            cc.site
                .map_or_else(|| "{closure}".to_string(), |s| s.render(&self.script_name)),
        )
    }

    pub fn is_closure(&self, v: &Value) -> bool {
        matches!(self.as_array(v), Some(PhpObj::Closure { .. }))
    }

    /// The closure's current private-access scope class, if any (for `bindTo`'s
    /// default "keep the current scope" behavior).
    fn closure_scope(&self, v: &Value) -> Option<String> {
        match self.as_array(v) {
            Some(PhpObj::Closure { scope, .. }) => scope.clone(),
            _ => None,
        }
    }

    /// Whether the closure was declared `static` (`ZEND_ACC_STATIC`).
    fn closure_is_static(&self, v: &Value) -> bool {
        matches!(
            self.as_array(v),
            Some(PhpObj::Closure {
                is_static: true,
                ..
            })
        )
    }

    /// Whether the declared class `lower` is one the reference ships
    /// (`ZEND_INTERNAL_CLASS`): an engine type, or a PHP-written prelude class
    /// standing in for one.
    fn is_internal_class(&self, lower: &str) -> bool {
        crate::prelude_type(lower).is_some() || !self.classes.contains_key(lower)
    }

    // ── objects / classes ──────────────────────────────────────────────────

    pub fn is_object(&self, v: &Value) -> bool {
        matches!(self.as_array(v), Some(PhpObj::Object { .. }))
    }

    /// The heap handle of any object/array/resource value — a stable per-instance
    /// id (`spl_object_id`). `None` for non-heap values.
    ///
    /// This is the same number `var_dump` prints as `#N`, as it is in PHP —
    /// both are the object's handle — so it is `object_ordinal`, not the raw
    /// heap index, which also counts arrays, closures and resources.
    pub fn object_id(&self, v: &Value) -> Option<i64> {
        match v {
            Value::Obj(_) if self.is_object(v) => Some(self.object_ordinal(v) as i64),
            Value::Obj(h) => Some(*h as i64),
            _ => None,
        }
    }

    /// The 1-based creation-order number PHP shows as `#N` in `var_dump`.
    ///
    /// PHP numbers class instances only, so count the `Object` entries up to and
    /// including this handle rather than using the raw heap index (which also
    /// covers arrays, closures and resources).
    ///
    /// KNOWN DIVERGENCE: PHP frees an object's handle when its refcount drops to
    /// zero and hands the number to the next allocation, so a program that
    /// discards an object sees a *lower* number afterwards than phplang, which
    /// allocates from an append-only arena and never frees. Closing this needs
    /// refcounted handles and a free list ordered the way PHP's allocator orders
    /// its own — substrate that does not exist here — so the numbers agree only
    /// while no object has become unreachable.
    pub fn object_ordinal(&self, v: &Value) -> usize {
        let Value::Obj(h) = v else { return 0 };
        self.objs
            .iter()
            .enumerate()
            .take(*h as usize + 1)
            .filter(|(i, o)| {
                matches!(o, PhpObj::Object { .. }) && !self.transient_objs.contains(&(*i as u32))
            })
            .count()
    }

    /// The class name of an object handle, or `None` if `v` is not an object.
    ///
    /// STRUCTURAL: only a class instance answers here. Every call site that asks
    /// "what does PHP think this value's class is" wants [`Self::instance_class`]
    /// instead — a closure and a generator are objects of class `Closure` and
    /// `Generator` to a PHP program, but neither is a `PhpObj::Object`.
    pub fn object_class(&self, v: &Value) -> Option<String> {
        match self.as_array(v) {
            Some(PhpObj::Object { class, .. }) => Some(class.clone()),
            _ => None,
        }
    }

    /// The class name PHP reports for `v`, or `None` when `v` is not an object at
    /// all (a scalar, an array, a resource).
    ///
    /// This is the reflection surface's view, and it is wider than
    /// [`Self::object_class`]: a closure is an instance of `Closure` and a
    /// generator an instance of `Generator`, so `get_class`, `::class`,
    /// `instanceof`, `is_a`, `method_exists`, `get_debug_type` and every
    /// diagnostic that names a value's type must go through here. Before this
    /// existed each of those either special-cased `is_closure` by hand or missed
    /// the case, which is why `get_class($gen)` was a `TypeError` reading
    /// "must be of type object, object given".
    pub fn instance_class(&self, v: &Value) -> Option<String> {
        match self.as_array(v) {
            Some(PhpObj::Object { class, .. }) => Some(class.clone()),
            Some(PhpObj::Closure { .. }) => Some("Closure".to_string()),
            Some(PhpObj::Generator { .. }) => Some("Generator".to_string()),
            _ => None,
        }
    }

    /// Whether `v` is an object to a PHP program — `is_object()`'s answer.
    /// True for class instances, closures and generators; false for arrays.
    pub fn is_object_value(&self, v: &Value) -> bool {
        matches!(
            self.as_array(v),
            Some(PhpObj::Object { .. } | PhpObj::Closure { .. } | PhpObj::Generator { .. })
        )
    }

    // ── reflection support (for the `reflection` stdlib module) ──────────────

    /// Whether a type of the given name is declared (case-insensitive) — a
    /// class, an interface, a trait or an enum. The kind is not distinguished
    /// here because most callers only need "is this name resolvable"; the
    /// reflection builtins ask [`Self::type_kind`] instead.
    pub fn class_exists(&self, name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        self.classes.contains_key(&lower) || builtin_type_kind(&lower).is_some()
    }

    /// Which of PHP's four declaration kinds `name` is, or `None` if the name is
    /// not declared. `class_exists`/`interface_exists`/`trait_exists`/
    /// `enum_exists` each answer for exactly one kind, so they cannot share the
    /// "is it declared at all" test: an `interface I {}` makes `class_exists('I')`
    /// FALSE in PHP even though the name resolves everywhere else.
    pub fn type_kind(&self, name: &str) -> Option<TypeKind> {
        let lower = name.to_ascii_lowercase();
        match self.classes.get(&lower) {
            Some(d) if d.is_trait => Some(TypeKind::Trait),
            Some(d) if d.is_interface => Some(TypeKind::Interface),
            Some(d) if d.is_enum => Some(TypeKind::Enum),
            Some(_) => Some(TypeKind::Class),
            None => builtin_type_kind(&lower),
        }
    }

    /// Every interface `class` implements, transitively, nearest first, in
    /// declared spelling. For `class_implements`.
    pub fn class_interface_names(&self, class: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        // Breadth-first over the parent chain and each class's interface list,
        // so a class reports the interfaces its ancestors implement too.
        let mut queue = vec![class.to_ascii_lowercase()];
        let mut seen: Vec<String> = Vec::new();
        while let Some(c) = queue.pop() {
            if seen.contains(&c) || seen.len() > 1000 {
                continue;
            }
            seen.push(c.clone());
            let Some(def) = self.classes.get(&c) else {
                // An engine type: its ancestry is the built-in table's, and
                // every ancestor there is an interface.
                for p in builtin_type_parents(&c) {
                    if !out.iter().any(|o| o.eq_ignore_ascii_case(p)) {
                        out.push((*p).to_string());
                    }
                }
                continue;
            };
            if let Some(p) = &def.parent {
                queue.push(p.to_ascii_lowercase());
            }
            for i in &def.interfaces {
                let spelled = self
                    .classes
                    .get(&i.to_ascii_lowercase())
                    .map(|d| d.name.clone())
                    .unwrap_or_else(|| i.clone());
                if !out.contains(&spelled) {
                    out.push(spelled);
                }
                queue.push(i.to_ascii_lowercase());
            }
        }
        out
    }

    /// The traits `class` composes with `use`, in source order and declared
    /// spelling. For `class_uses`, which reports only THIS class's `use` clauses
    /// (PHP does not walk the parent chain for it).
    pub fn class_trait_names(&self, class: &str) -> Vec<String> {
        self.classes
            .get(&class.to_ascii_lowercase())
            .map(|d| d.uses.clone())
            .unwrap_or_default()
    }

    /// The PHP fatal-error message if `class` cannot be instantiated with `new`
    /// (it is `abstract`, an `interface`, a `trait` or an `enum`), else `None`. The original casing is
    /// used in the message.
    fn class_instantiation_error(&self, class: &str) -> Option<String> {
        let def = self.classes.get(&class.to_ascii_lowercase())?;
        // Four kinds cannot be instantiated, and the reference names each by
        // its OWN keyword so a caller catching the `Error` can tell them apart
        let kind = if def.is_interface {
            "interface"
        } else if def.is_trait {
            "trait"
        } else if def.is_enum {
            "enum"
        } else if def.is_abstract {
            "abstract class"
        } else {
            return None;
        };
        Some(format!(
            "Cannot instantiate {kind} {}",
            display_class(class)
        ))
    }

    /// Whether a user function of the given name is defined (case-insensitive).
    pub fn function_defined(&self, name: &str) -> bool {
        self.functions.contains_key(&name.to_ascii_lowercase())
    }

    /// The declared type of property `name` as seen from `class`, with the class
    /// (display spelling) that declares it — the nearest along the parent chain.
    pub fn prop_type(&self, class: &str, name: &str) -> Option<(String, TypeHint)> {
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let def = self.classes.get(&c)?;
            if let Some(ty) = def.prop_types.get(name) {
                return Some((def.name.clone(), ty.clone()));
            }
            if def.prop_vis.contains_key(name) {
                return None;
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        None
    }

    /// The declared parent-class name of `name`, or `None` (no parent / unknown).
    pub fn class_parent(&self, name: &str) -> Option<String> {
        self.classes
            .get(&name.to_ascii_lowercase())
            .and_then(|d| d.parent.clone())
    }

    /// Whether `class` is `target` or descends from it (case-insensitive);
    /// `target == "Throwable"` matches either exception root. The public form of
    /// the catch-matching walk, for `is_a`/`is_subclass_of`/`instanceof`.
    pub fn is_a_class(&self, class: &str, target: &str) -> bool {
        self.catch_matches(class, target)
    }

    /// Method names visible on `class`, walking the parent chain (lowercased, as
    /// stored). For `get_class_methods`.
    /// `get_class_methods`: the methods `class` has, in its function table's
    /// order — its own and its traits', then each ancestor's that it does not
    /// override — keeping those the calling scope may call
    /// (`zend_check_method_accessible`). DIVERGENCE: a name is lowercased
    /// rather than spelled as declared — `tests/stdlib_reflection.rs` pins that
    /// (see BUGS.md). An enum's synthesized `cases`, `from`
    /// and `tryFrom` follow its own methods. The prelude's helpers, which the
    /// internal classes it stands in for do not have, are left out.
    pub fn class_methods_visible(&self, class: &str) -> Vec<String> {
        let scope = self.current_class_ctx().map(|s| s.to_ascii_lowercase());
        let mut seen: Vec<String> = Vec::new();
        let mut out = Vec::new();
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let Some(def) = self.classes.get(&c) else {
                out.extend(builtin_type_methods(&c).iter().map(|m| m.to_string()));
                break;
            };
            let mut names: Vec<String> = def.method_sites.iter().map(|(n, _)| n.clone()).collect();
            if def.is_enum {
                names.push("cases".to_string());
                if def
                    .interfaces
                    .iter()
                    .any(|i| i.eq_ignore_ascii_case("BackedEnum"))
                {
                    names.push("from".to_string());
                    names.push("tryFrom".to_string());
                }
            }
            for name in names {
                let lname = name.to_ascii_lowercase();
                if seen.contains(&lname) {
                    continue;
                }
                seen.push(lname.clone());
                if crate::prelude_type(&c).is_some()
                    && is_prelude_helper_name(&format!("{c}::{name}"))
                {
                    continue;
                }
                let accessible = match def.method_vis.get(&lname) {
                    Some(Visibility::Private) => scope.as_deref() == Some(c.as_str()),
                    Some(Visibility::Protected) => scope
                        .as_deref()
                        .is_some_and(|s| self.class_is_a(s, &c) || self.class_is_a(&c, s)),
                    _ => true,
                };
                if accessible {
                    out.push(lname);
                }
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        out
    }

    pub fn class_method_names(&self, class: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let Some(def) = self.classes.get(&c) else {
                // An engine type has no `ClassDef`; its methods are listed in
                // `BUILTIN_TYPES`, in PHP's own spelling.
                out.extend(builtin_type_methods(&c).iter().map(|m| m.to_string()));
                break;
            };
            for m in def.methods.keys() {
                if !out.contains(m) {
                    out.push(m.clone());
                }
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        out
    }

    /// Whether `class` (or an ancestor) defines `method` (case-insensitive).
    pub fn class_has_method(&self, class: &str, method: &str) -> bool {
        if self.method_declared(class, &method.to_ascii_lowercase()) {
            return true;
        }
        // An engine type (`Generator`, `Closure`, `Iterator`, …) has no
        // `ClassDef` for `resolve_method` to walk.
        builtin_type_methods(&class.to_ascii_lowercase())
            .iter()
            .any(|m| m.eq_ignore_ascii_case(method))
    }

    /// Whether `class` (or an ancestor) declares the property `name`.
    pub fn class_has_prop(&self, class: &str, name: &str) -> bool {
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let Some(def) = self.classes.get(&c) else {
                break;
            };
            if def.prop_defaults.iter().any(|(n, _)| n == name) {
                return true;
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        false
    }

    /// How `print_r` and `serialize` decorate a property name that is not public:
    /// `print_r` writes `[b:protected]` / `[c:Declaring:private]`, and `serialize`
    /// writes `\0*\0b` / `\0Declaring\0c`. Both need the DECLARING class in its
    /// source spelling, which is why this returns the name rather than a flag.
    ///
    /// `None` for a public or undeclared (dynamic) property — the two cases that
    /// print bare.
    pub fn prop_visibility(&self, class: &str, name: &str) -> Option<(String, Visibility)> {
        let (declaring, vis) = self.resolve_prop_vis(class, name)?;
        if matches!(vis, Visibility::Public) {
            return None;
        }
        let spelled = self
            .classes
            .get(&declaring)
            .map(|d| d.name.clone())
            .unwrap_or(declaring);
        Some((spelled, vis))
    }

    /// An `enum` case singleton's `(case name, backing value)`; the value is
    /// `None` for a pure enum. `None` if `v` is not an enum case at all.
    ///
    /// Every renderer needs this: an enum does not print like the object it is
    /// implemented as (`var_dump` writes `enum(Suit::Hearts)`, `json_encode`
    /// writes the backing value, `serialize` writes `E:9:"Suit:Hearts";`).
    pub fn enum_case_of(&self, v: &Value) -> Option<(String, Option<Value>)> {
        let Some(PhpObj::Object { class, props }) = self.as_array(v) else {
            return None;
        };
        if !self.is_enum_class(class) {
            return None;
        }
        let name = props.get("name").map(|n| self.to_str(n))?;
        Some((name, props.get("value").map(|x| self.deref(x.clone()))))
    }

    /// An object's `(name, value)` properties in insertion order. For
    /// `get_object_vars`; empty if `v` is not an object.
    pub fn object_props(&self, v: &Value) -> Vec<(String, Value)> {
        match self.as_array(v) {
            Some(PhpObj::Object { props, .. }) => props
                .iter()
                .map(|(k, v)| (k.clone(), self.deref(v.clone())))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// An object's properties that are *visible from the current scope* — what
    /// `get_object_vars` returns. Called from outside the class, that is the
    /// public ones only; called from inside a method of the class, the private
    /// and protected ones as well.
    pub fn object_props_visible(&self, v: &Value) -> Vec<(String, Value)> {
        let Some(class) = self.object_class(v) else {
            return Vec::new();
        };
        self.object_props(v)
            .into_iter()
            .filter(|(name, _)| match self.resolve_prop_vis(&class, name) {
                Some((declaring, vis)) => self.visibility_allows(vis, &declaring),
                // A dynamic property was never declared, so it is public.
                None => true,
            })
            .collect()
    }

    /// An object's properties keyed the way `(array)` spells them: PHP mangles a
    /// non-public name so the cast cannot silently collide two properties of the
    /// same name from different visibilities. A private `$p` declared by `C`
    /// becomes `"\0C\0p"`, a protected one `"\0*\0p"`, and a public one stays as
    /// it is. The NUL bytes are why such a key cannot be reached with `$arr['p']`.
    pub fn object_props_mangled(&self, v: &Value) -> Vec<(String, Value)> {
        let class = self.object_class(v).unwrap_or_default();
        self.object_props(v)
            .into_iter()
            .map(|(name, val)| {
                let key = match self.declaring_class_of_prop(&class, &name) {
                    // The mangled name carries the declaring class *as written*.
                    Some((declaring, Visibility::Private)) => format!("\0{declaring}\0{name}"),
                    Some((_, Visibility::Protected)) => format!("\0*\0{name}"),
                    _ => name,
                };
                (key, val)
            })
            .collect()
    }

    /// [`resolve_prop_vis`] with the declaring class in its source spelling,
    /// which is the form `(array)`'s mangled key needs. Class definitions are
    /// keyed by lowercase name but the `parent` link keeps its original case, so
    /// the walk carries the declared name alongside the lookup key.
    fn declaring_class_of_prop(&self, class: &str, name: &str) -> Option<(String, Visibility)> {
        let mut cur = Some(class.to_string());
        while let Some(c) = cur {
            let def = self.classes.get(&c.to_ascii_lowercase())?;
            if let Some(v) = def.prop_vis.get(name) {
                return Some((c, *v));
            }
            cur = def.parent.clone();
        }
        None
    }

    /// The object's `(name, value, is_reference)` triples — the property form of
    /// `array_pairs_marked`, for `var_dump`'s `&` marker.
    pub fn object_props_marked(&self, v: &Value) -> Vec<(String, Value, bool)> {
        match self.as_array(v) {
            Some(PhpObj::Object { props, .. }) => props
                .iter()
                .map(|(k, v)| {
                    let is_ref = self.ref_slot_of_value(v).is_some();
                    (k.clone(), self.deref(v.clone()), is_ref)
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    // ── stream resources ─────────────────────────────────────────────────────

    /// Allocate a stream resource, numbering it from the resource counter.
    /// `php://temp` takes two numbers — the reference opens an inner memory
    /// stream behind it — and the outer stream gets the first.
    pub fn new_stream(&mut self, mut s: crate::stdlib::fileres::Stream) -> Value {
        s.id = self.next_resource_id;
        self.next_resource_id += if s.kind == crate::stdlib::fileres::StreamKind::Temp {
            2
        } else {
            1
        };
        self.objs.push(PhpObj::Resource(Box::new(s)));
        Value::Obj((self.objs.len() - 1) as u32)
    }

    /// The stream behind `v`, open or closed.
    pub fn stream_any(&self, v: &Value) -> Option<&crate::stdlib::fileres::Stream> {
        match self.as_array(v) {
            Some(PhpObj::Resource(s)) => Some(s),
            _ => None,
        }
    }

    /// The OPEN stream behind `v`; a closed one answers `None`.
    pub fn stream(&self, v: &Value) -> Option<&crate::stdlib::fileres::Stream> {
        self.stream_any(v).filter(|s| !s.closed)
    }

    /// The open stream behind `v`, mutably.
    pub fn stream_mut(&mut self, v: &Value) -> Option<&mut crate::stdlib::fileres::Stream> {
        match self.as_array_mut(v) {
            Some(PhpObj::Resource(s)) if !s.closed => Some(s),
            _ => None,
        }
    }

    /// Write into a buffered stream. A `php://temp` stream that reaches its
    /// `maxmemory` moves to a temporary file in the reference, which opens one
    /// more resource: the number is consumed here so later ones line up.
    pub fn stream_write(&mut self, v: &Value, bytes: &[u8]) {
        let mut spilled = false;
        if let Some(s) = self.stream_mut(v) {
            if let Some(limit) = s.spill_at {
                if s.buf.len() + bytes.len() >= limit {
                    s.spill_at = None;
                    spilled = true;
                }
            }
            s.write_at_cursor(bytes);
        }
        if spilled {
            self.next_resource_id += 1;
        }
    }

    /// Spend a resource number without keeping a stream: what a whole-file
    /// function does when it opens, reads and closes one internally.
    pub fn consume_resource_id(&mut self) {
        self.next_resource_id += 1;
    }

    /// `(resource number, closed)` for a stream value.
    pub fn resource_info(&self, v: &Value) -> Option<(i64, bool)> {
        self.stream_any(v).map(|s| (s.id, s.closed))
    }

    /// Whether `v` is an open resource (`is_resource`).
    pub fn is_resource(&self, v: &Value) -> bool {
        self.stream(v).is_some()
    }

    /// A stream resource, open or closed: a closed one is still of type
    /// resource, just no longer usable.
    pub fn is_stream(&self, v: &Value) -> bool {
        self.stream_any(v).is_some()
    }

    /// Write to the process's standard output, past any output buffering —
    /// what `fwrite(STDOUT, …)` does. An embedding host's capture buffer still
    /// receives it, since that buffer IS its standard output.
    pub fn write_stdout_direct(&mut self, s: &str) {
        if let Some(buf) = &mut self.capture {
            buf.push_str(s);
        } else {
            use std::io::Write;
            let _ = std::io::stdout().write_all(s.as_bytes());
        }
    }

    /// `$obj->name` read (`Undef` if the object lacks the property).
    /// Whether the object `recv` carries a property called `name` (declared
    /// or dynamic, initialized or not), whatever its visibility.
    pub fn obj_has_prop(&self, recv: &Value, name: &str) -> bool {
        matches!(self.as_array(recv), Some(PhpObj::Object { props, .. }) if props.contains_key(name))
    }

    pub fn prop_get(&self, recv: &Value, name: &str) -> Value {
        match self.as_array(recv) {
            Some(PhpObj::Object { props, .. }) => {
                let raw = props.get(name).cloned().unwrap_or(Value::Undef);
                self.deref(raw)
            }
            _ => Value::Undef,
        }
    }

    /// `$obj->name = val` — mutates the shared instance behind the handle. A
    /// property a `&` binding has turned into a reference is written through its
    /// cell rather than replaced, so the alias observes the write.
    pub fn prop_set(&mut self, recv: &Value, name: &str, val: Value) {
        if let Some(slot) = self.prop_ref_slot(recv, name) {
            self.ref_cell_set(slot, val);
            return;
        }
        self.obj_put_prop(recv, name, val);
    }

    /// Why a property WRITE on `recv` is refused, or `None` when `recv` is an
    /// object that can take it. `verb` names the write the way the reference's
    /// message does: `assign`, `increment/decrement`, or `modify` for a fetch
    /// that goes on to write deeper (`$x->p[] = 1`, `$x->p->q = 1`).
    ///
    /// A closure and a generator are objects that refuse every dynamic property;
    /// anything else that is not an object names its type — a bool by its value,
    /// as `zend_zval_value_name` spells it.
    pub fn prop_write_refusal(&self, recv: &Value, name: &str, verb: &str) -> Option<String> {
        let ty = match recv {
            Value::Bool(true) => "true",
            Value::Bool(false) => "false",
            Value::Obj(_) => match self.as_array(recv) {
                Some(PhpObj::Object { .. } | PhpObj::Ref { .. }) | None => return None,
                Some(PhpObj::Closure { .. }) => {
                    return Some(format!("Cannot create dynamic property Closure::${name}"));
                }
                Some(PhpObj::Generator { .. }) => {
                    return Some(format!("Cannot create dynamic property Generator::${name}"));
                }
                Some(PhpObj::Resource(_)) => "resource",
                Some(PhpObj::Array { .. }) => "array",
            },
            other => self.diag_type(other),
        };
        Some(format!("Attempt to {verb} property \"{name}\" on {ty}"))
    }

    /// `$obj->name = val` from PHP source, with the PHP 8.2 deprecation for
    /// creating a property the class never declared.
    ///
    /// Split from `prop_set` because the engine's own writes must NOT warn:
    /// `seed_throwable` stamps `file`/`line`/`trace` onto every Throwable, and an
    /// internal write is not the user creating a dynamic property. Only the
    /// opcodes that lower a source-level `->` assignment come through here.
    pub fn prop_set_checked(&mut self, recv: &Value, name: &str, val: Value) {
        self.warn_dynamic_prop(recv, name);
        self.prop_set(recv, name, val);
    }

    /// Raise `Creation of dynamic property C::$p is deprecated` if writing `name`
    /// on `recv` would create one.
    ///
    /// It creates one when the object does not already carry the property AND no
    /// class in its chain declares it. `stdClass` is exempt — it is PHP's
    /// property bag, and `(object)` casts and `json_decode` produce it — as is any
    /// class opted in with `#[AllowDynamicProperties]`, itself or inherited.
    pub fn warn_dynamic_prop(&mut self, recv: &Value, name: &str) {
        let Some(PhpObj::Object { class, props }) = self.as_array(recv) else {
            return;
        };
        if props.contains_key(name) || class.eq_ignore_ascii_case("stdClass") {
            return;
        }
        let class = class.clone();
        if self.prop_is_declared(&class, name) || self.allows_dynamic_props(&class) {
            return;
        }
        self.deprecated(format_args!(
            "Creation of dynamic property {class}::${name} is deprecated"
        ));
    }

    /// Whether `class` or any ancestor declares a property called `name`.
    /// A declaration with no initializer still counts, so `prop_vis` — which every
    /// declaration writes — is the authority, not the defaults list.
    fn prop_is_declared(&self, class: &str, name: &str) -> bool {
        self.class_chain(class).any(|d| {
            d.prop_vis.contains_key(name) || d.prop_defaults.iter().any(|(p, _)| p == name)
        })
    }

    /// Whether `class` or any ancestor carries `#[AllowDynamicProperties]`.
    fn allows_dynamic_props(&self, class: &str) -> bool {
        self.class_chain(class).any(|d| d.allow_dynamic_props)
    }

    /// The class and its ancestors, innermost first. Stops at an undeclared
    /// parent rather than looping, so a broken chain cannot hang a lookup.
    fn class_chain<'a>(&'a self, class: &str) -> impl Iterator<Item = &'a ClassDef> + 'a {
        let mut cur = Some(class.to_ascii_lowercase());
        std::iter::from_fn(move || {
            let def = self.classes.get(&cur.take()?)?;
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
            Some(def)
        })
    }

    /// The reference slot `$obj->name` holds, if that property is a reference.
    fn prop_ref_slot(&self, recv: &Value, name: &str) -> Option<usize> {
        let Some(PhpObj::Object { props, .. }) = self.as_array(recv) else {
            return None;
        };
        self.ref_slot_of_value(props.get(name)?)
    }

    /// Ensure `$obj->name` holds an array and return its handle, creating an empty
    /// array (and storing it back on the object) if the property is unset or not an
    /// array. The pivot for indexing/appending into an array-valued property
    /// (`$this->items[] = x`, `$this->map[$k] = v`).
    pub fn prop_ensure_array(&mut self, recv: &Value, name: &str) -> Value {
        let cur = self.prop_get(recv, name);
        if self.is_array(&cur) {
            return cur;
        }
        let arr = self.new_array();
        self.prop_set(recv, name, arr.clone());
        arr
    }

    /// Whether `class` or an ancestor declares `method` (already lowercased),
    /// WITHOUT materializing the definition.
    ///
    /// [`PhpHost::resolve_method`] clones the whole [`FuncDef`] — including the
    /// method body's `Chunk`, which owns its ops, constants, names, line table
    /// and nested sub-chunks — so asking it a yes/no question copies an entire
    /// compiled method to throw it away. Every ordinary `$o->m()` asked twice:
    /// once through [`PhpHost::method_dispatch`] to decide whether the call is
    /// allowed, then again in [`call_method`] for the definition it actually
    /// runs. This answers the first question with a `contains_key` per class in
    /// the chain and no allocation, leaving one clone on the path.
    fn method_declared(&self, class: &str, method: &str) -> bool {
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let Some(def) = self.classes.get(&c) else {
                return false;
            };
            if def.methods.contains_key(method) {
                return true;
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        false
    }

    /// The by-reference parameter at 1-based `argno` of method `method` on
    /// `recv` (an object, or a class name), as `(Class::method, parameter)`, or
    /// `None` when the method does not resolve or that parameter is by value.
    /// The class named is the one that DECLARES the method. A variadic tail is
    /// left alone.
    pub fn method_byref_param(
        &self,
        recv: &Value,
        method: &str,
        argno: usize,
    ) -> Option<(String, String)> {
        let class = match recv {
            Value::Obj(_) => self.object_class(recv)?,
            other => self.to_str(other),
        };
        let (decl, def) = self.resolve_method(&class, &method.to_ascii_lowercase())?;
        let p = def.params.get(argno.checked_sub(1)?)?;
        (p.by_ref && !p.variadic).then(|| {
            (
                format!("{}::{method}", self.class_display_name(&decl)),
                p.name.clone(),
            )
        })
    }

    /// The lowercased class declaring the method `obj` runs for `lname`.
    pub(crate) fn method_owner(&self, obj: &Value, lname: &str) -> Option<String> {
        let class = self.instance_class(obj)?;
        self.resolve_method(&class, &lname.to_ascii_lowercase())
            .map(|(c, _)| c)
    }

    /// Resolve a method by walking the class up its parent chain; returns the
    /// defining class name plus the method definition.
    fn resolve_method(&self, class: &str, method: &str) -> Option<(String, FuncDef)> {
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let def = self.classes.get(&c)?;
            if let Some(m) = def.methods.get(method) {
                return Some((c, m.clone()));
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        None
    }

    /// The class whose method is currently executing (for visibility checks),
    /// derived from the innermost frame's `Class::method` name. `None` in the
    /// global scope, a free function, or a closure (frame names without `::`).
    fn current_class_ctx(&self) -> Option<String> {
        if let Some(c) = self.init_class_scope.last() {
            return Some(c.clone());
        }
        let name = self.scopes.last()?.name.as_ref()?;
        name.rsplit_once("::").map(|(cls, _)| cls.to_string())
    }

    /// `__CLASS__` for the frame that is running, used where the parse could not
    /// name the class: a trait method (whose `__CLASS__` is the class that USED
    /// the trait) and an anonymous class (whose name the compiler mints).
    ///
    /// The DECLARED spelling is returned, and the whole of it — an anonymous
    /// class keeps the NUL-separated unique tail, which is what `get_class`
    /// reports for it too, and `__CLASS__` and `get_class($this)` agree in the
    /// reference.
    pub fn magic_class(&self) -> String {
        let Some(cls) = self.current_class_ctx() else {
            return String::new();
        };
        self.classes.get(&cls).map_or(cls, |d| d.name.clone())
    }

    /// The parent of the class the running frame belongs to, in its declared
    /// spelling. Empty when there is no frame class or it extends nothing.
    pub fn magic_parent_class(&self) -> String {
        let Some(cls) = self.current_class_ctx() else {
            return String::new();
        };
        // Lowercased before the lookup: `current_class_ctx` can hand back the
        // DECLARED spelling (a closure bound with `->call()` does), and the
        // class map is keyed by the lowercased name.
        let Some(parent) = self
            .classes
            .get(&cls.to_ascii_lowercase())
            .and_then(|d| d.parent.clone())
        else {
            return String::new();
        };
        self.classes
            .get(&parent.to_ascii_lowercase())
            .map_or(parent, |d| d.name.clone())
    }

    /// `__DIR__` — the directory the script lives in.
    ///
    /// `php -r` code has no file, and the reference answers the working directory
    /// for it; the same goes for a script read from standard input. Both are
    /// recognised by [`PhpHost::script_name`] not being a path.
    pub fn magic_dir(&self) -> String {
        let name = self.current_file();
        if name.starts_with('/') {
            if let Some(parent) = std::path::Path::new(name).parent() {
                return parent.display().to_string();
            }
        }
        std::env::current_dir().map_or_else(|_| String::new(), |p| p.display().to_string())
    }

    /// The class (lowercased) that declared `name` `readonly`, walking up from
    /// `class`. `None` when the property is not readonly anywhere in the chain,
    /// which is the overwhelmingly common case and the only one that costs
    /// nothing on the write path.
    fn readonly_owner(&self, class: &str, name: &str) -> Option<String> {
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let def = self.classes.get(&c)?;
            if def.readonly_props.contains(name) {
                return Some(c);
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        None
    }

    /// Why a write to `$recv->name` is refused, or `None` when it is allowed.
    ///
    /// A readonly property takes exactly one write, and PHP 8.4 gave the rule
    /// its `protected(set)` shape: the initializing write must come from the
    /// declaring class or a subclass, and every later write is refused from
    /// everywhere. `__clone` is the one documented reopening — a copy is
    /// allowed to be given a fresh identity — so a write there always passes.
    ///
    /// The two refusals have different messages, and which one applies turns on
    /// whether the property was ever initialized, not on its current value: a
    /// readonly property explicitly set to null is initialized.
    pub fn readonly_write_error(&self, recv: &Value, name: &str) -> Option<String> {
        let Some(PhpObj::Object { class, .. }) = self.as_array(recv) else {
            return None;
        };
        let owner = self.readonly_owner(class, name)?;
        if let (Value::Obj(id), Some(c)) = (recv, self.cloning) {
            if *id == c {
                return None;
            }
        }
        let display = self.class_display_name(&owner);
        if self.readonly_is_init(recv, name) {
            return Some(format!(
                "Cannot modify readonly property {display}::${name}"
            ));
        }
        let scope = self.current_class_ctx();
        if scope
            .as_deref()
            .is_some_and(|s| self.class_is_a(&s.to_ascii_lowercase(), &owner))
        {
            return None;
        }
        let from = match &scope {
            Some(s) => format!("scope {}", self.class_display_name(&s.to_ascii_lowercase())),
            None => "global scope".to_string(),
        };
        Some(format!(
            "Cannot modify protected(set) readonly property {display}::${name} from {from}"
        ))
    }

    /// Why `unset($recv->name)` is refused, or `None`. PHP allows unsetting a
    /// readonly property that was never initialized (it is still uninitialized
    /// afterwards) and refuses it once it holds a value.
    pub fn readonly_unset_error(&self, recv: &Value, name: &str) -> Option<String> {
        let Some(PhpObj::Object { class, .. }) = self.as_array(recv) else {
            return None;
        };
        let owner = self.readonly_owner(class, name)?;
        self.readonly_is_init(recv, name).then(|| {
            let display = self.class_display_name(&owner);
            format!("Cannot unset readonly property {display}::${name}")
        })
    }

    /// Why `$recv->name[…] = …` (or any other write *through* the property) is
    /// refused, or `None`. PHP will not hand out a modifiable reference to a
    /// readonly property at all, initialized or not, and says so in its own
    /// wording rather than the plain "cannot modify".
    pub fn readonly_indirect_error(&self, recv: &Value, name: &str) -> Option<String> {
        let Some(PhpObj::Object { class, .. }) = self.as_array(recv) else {
            return None;
        };
        if let (Value::Obj(id), Some(c)) = (recv, self.cloning) {
            if *id == c {
                return None;
            }
        }
        let owner = self.readonly_owner(class, name)?;
        // The initializing write inside the declaring scope goes through the
        // ordinary path; only a write to an already-set property is "indirect"
        // in the sense the message means.
        if !self.readonly_is_init(recv, name) {
            return None;
        }
        let display = self.class_display_name(&owner);
        Some(format!(
            "Cannot indirectly modify readonly property {display}::${name}"
        ))
    }

    /// Whether this object's readonly property `name` has taken its one write.
    ///
    /// Tracked explicitly rather than inferred from the stored value, because
    /// null is a perfectly good value for an initialized readonly property and
    /// is indistinguishable from "never written" in the property table.
    fn readonly_is_init(&self, recv: &Value, name: &str) -> bool {
        let Value::Obj(id) = recv else { return false };
        self.readonly_init
            .get(id)
            .is_some_and(|set| set.contains(name))
    }

    /// Record that `$recv->name` has taken its one readonly write. Called after
    /// the write, by the same opcode handlers that asked permission for it.
    pub fn readonly_note_init(&mut self, recv: &Value, name: &str) {
        let Value::Obj(id) = recv else { return };
        let Some(PhpObj::Object { class, .. }) = self.as_array(recv) else {
            return;
        };
        if self.readonly_owner(class, name).is_none() {
            return;
        }
        self.readonly_init
            .entry(*id)
            .or_default()
            .insert(name.to_string());
    }

    /// Resolve the declared visibility of property `name` on `class`, walking the
    /// parent chain; returns the declaring class (lowercased) and its visibility.
    /// `None` for a dynamic/undeclared property (treated as public).
    fn resolve_prop_vis(&self, class: &str, name: &str) -> Option<(String, Visibility)> {
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let def = self.classes.get(&c)?;
            if let Some(v) = def.prop_vis.get(name) {
                return Some((c, *v));
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        None
    }

    /// Whether the current class context may access a member declared with
    /// `vis` in `declaring` (lowercased). Public: always. Private: only from the
    /// declaring class itself. Protected: from the declaring class or any class in
    /// the same inheritance line (ancestor or descendant).
    fn visibility_allows(&self, vis: Visibility, declaring: &str) -> bool {
        match vis {
            Visibility::Public => true,
            Visibility::Private => {
                matches!(self.current_class_ctx(), Some(c) if c.eq_ignore_ascii_case(declaring))
            }
            Visibility::Protected => match self.current_class_ctx() {
                Some(c) => self.class_is_a(&c, declaring) || self.class_is_a(declaring, &c),
                None => false,
            },
        }
    }

    /// Decide what `$obj->name` does for the operation whose magic method is
    /// `magic` (`__get`, `__set`, `__isset`, `__unset`).
    ///
    /// The order is the reference's, and the two halves are easy to get backwards:
    /// a magic method is consulted for a property that is not THERE just as much
    /// as for one that is out of reach, and it is consulted BEFORE any access
    /// error. So `unset($o->pub)` on a class with `__get` really does route later
    /// reads of that public property through `__get` — the slot is gone, and gone
    /// is indistinguishable from never-declared at this point.
    ///
    /// A magic method already running for this object and property is skipped,
    /// which is the recursion guard described on `magic_in_progress`.
    pub fn prop_access(&self, recv: &Value, name: &str, magic: &'static str) -> PropAccess {
        let Some(PhpObj::Object { class, props }) = self.as_array(recv) else {
            // Not an object: no visibility to enforce and no magic to call.
            return PropAccess::Absent;
        };
        let declared = self.resolve_prop_vis(class, name);
        // A property no class declares carries no visibility, so it is reachable.
        let reachable = match &declared {
            Some((declaring, vis)) => self.visibility_allows(*vis, declaring),
            None => true,
        };
        if reachable && props.contains_key(name) {
            return PropAccess::Direct;
        }
        let handle = match recv {
            Value::Obj(h) => *h,
            _ => return PropAccess::Absent,
        };
        let guarded = self
            .magic_in_progress
            .iter()
            .any(|(h, p, m)| *h == handle && p == name && *m == magic);
        if !guarded && self.class_has_method(class, magic) {
            return PropAccess::Magic;
        }
        match declared {
            // Out of reach rather than absent — the reference names the
            // visibility and the class that declared it, not the class of the
            // object, which for an inherited private property is not the same.
            Some((declaring, vis)) if !reachable => {
                let vname = match vis {
                    Visibility::Private => "private",
                    Visibility::Protected => "protected",
                    Visibility::Public => unreachable!("public is always reachable"),
                };
                let class = self.class_display_name(&declaring);
                PropAccess::Denied(format!("Cannot access {vname} property {class}::${name}"))
            }
            _ => PropAccess::Absent,
        }
    }

    /// Mark a magic property access as in progress; the matching
    /// [`magic_leave`](PhpHost::magic_leave) must follow it.
    pub fn magic_enter(&mut self, recv: &Value, name: &str, magic: &'static str) {
        if let Value::Obj(h) = recv {
            self.magic_in_progress.push((*h, name.to_string(), magic));
        }
    }

    pub fn magic_leave(&mut self) {
        self.magic_in_progress.pop();
    }

    /// The open magic-access count, for a caller that has to put it back after an
    /// unwind — the same problem, and the same fix, as `@expr` suppression.
    pub fn magic_depth(&self) -> usize {
        self.magic_in_progress.len()
    }

    pub fn magic_restore(&mut self, depth: usize) {
        self.magic_in_progress.truncate(depth);
    }

    /// A class name as declared, recovered from the lowercased key the class
    /// table is indexed by, so a diagnostic prints `MyClass` and not `myclass`.
    pub fn class_display_name(&self, lowered: &str) -> String {
        let name = self
            .classes
            .get(lowered)
            .map_or(lowered, |d| d.name.as_str());
        display_class(name).to_string()
    }

    /// Remove `$obj->name` from the object's property table — `unset($o->p)`.
    /// A property that is not there is not an error; the caller has already
    /// decided this is the right thing to do (see [`prop_access`]).
    ///
    /// [`prop_access`]: PhpHost::prop_access
    pub fn prop_remove(&mut self, recv: &Value, name: &str) {
        self.note_unset(recv, name);
        if let Some(PhpObj::Object { props, .. }) = self.as_array_mut(recv) {
            props.shift_remove(name);
        }
    }

    /// Enforce method visibility for `$obj->method()`. Same policy as properties;
    /// the message matches PHP's `Call to <vis> method C::m() from <scope>`.
    pub fn check_method_access(&self, class: &str, method: &str) -> Result<(), String> {
        let Some((declaring, vis)) = self.declaring_method_vis(class, method) else {
            return Ok(());
        };
        if self.visibility_allows(vis, &declaring) {
            return Ok(());
        }
        let vname = match vis {
            Visibility::Private => "private",
            Visibility::Protected => "protected",
            Visibility::Public => unreachable!(),
        };
        let scope = match self.current_class_ctx() {
            Some(c) => format!("scope {c}"),
            None => "global scope".to_string(),
        };
        Err(format!(
            "Call to {vname} method {class}::{method}() from {scope}"
        ))
    }

    /// The class up `class`'s chain that DECLARED `method`, and the visibility it
    /// declared it with. `None` when no class on the chain declares it — which is
    /// how an inherited-but-undeclared method stays reachable.
    fn declaring_method_vis(&self, class: &str, method: &str) -> Option<(String, Visibility)> {
        let method_l = method.to_ascii_lowercase();
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let def = self.classes.get(&c)?;
            if let Some(v) = def.method_vis.get(&method_l) {
                return Some((c, *v));
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        None
    }

    /// The visibility `class` (or an ancestor) declared `method` with, as the
    /// reference spells it in a message. `None` for a public or undeclared one.
    pub fn method_visibility_name(&self, class: &str, method: &str) -> Option<&'static str> {
        match self.declaring_method_vis(class, method) {
            Some((_, Visibility::Private)) => Some("private"),
            Some((_, Visibility::Protected)) => Some("protected"),
            _ => None,
        }
    }

    /// Whether `class` (or an ancestor) declared `method` `static`.
    pub fn method_is_static(&self, class: &str, method: &str) -> bool {
        let method_l = method.to_ascii_lowercase();
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let Some(def) = self.classes.get(&c) else {
                return false;
            };
            if def.methods.contains_key(&method_l) {
                return def.static_methods.contains(&method_l);
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        false
    }

    /// The magic catch-all a call to `class::method` would fall back to, if any:
    /// `__call` for an instance call, `__callStatic` for a static one. A class
    /// with only `__call` does NOT answer a static call, and vice versa.
    fn magic_call_name(&self, class: &str, has_this: bool) -> Option<&'static str> {
        let magic = if has_this { "__call" } else { "__callstatic" };
        self.method_declared(class, magic).then_some(if has_this {
            "__call"
        } else {
            "__callStatic"
        })
    }

    /// Decide what `$obj->m(...)` (or `C::m(...)`) does, consulting the method
    /// table, the visibility rules and the magic catch-all in the reference's
    /// order. Every call opcode routes through this so the instance and static
    /// forms answer identically.
    pub fn method_dispatch(&self, class: &str, method: &str, has_this: bool) -> MethodDispatch {
        let method_l = method.to_ascii_lowercase();
        if !self.method_declared(class, &method_l) {
            return match self.magic_call_name(class, has_this) {
                Some(_) => MethodDispatch::Magic,
                None => MethodDispatch::Undefined,
            };
        }
        match self.check_method_access(class, method) {
            Ok(()) => MethodDispatch::Direct,
            // Out of reach, but a catch-all takes precedence over the access
            // error — the reference calls `__call` for a private method too.
            Err(msg) => match self.magic_call_name(class, has_this) {
                Some(_) => MethodDispatch::Magic,
                None => MethodDispatch::Denied(msg),
            },
        }
    }

    /// The ordered property-default initializer chunks for a class, parent props
    /// first and child declarations overriding by name, each with the class that
    /// declares it (its initializer runs in that class's scope). `None` if
    /// unknown class.
    /// The third element is whether the property starts UNINITIALIZED (typed,
    /// no default, as its final declaration along the chain says): its chunk is
    /// not run and the instance is created without it.
    #[allow(clippy::type_complexity)]
    fn class_prop_default_chunks(
        &self,
        class: &str,
    ) -> Option<Vec<(String, (String, Chunk, bool))>> {
        let cl = class.to_ascii_lowercase();
        if !self.classes.contains_key(&cl) {
            return None;
        }
        // Build the chain child → root, then apply root → child so a child's
        // redeclared default wins while parent props keep their leading position.
        let mut chain: Vec<(String, &ClassDef)> = Vec::new();
        let mut cur = Some(cl);
        while let Some(c) = cur {
            let Some(def) = self.classes.get(&c) else {
                break;
            };
            chain.push((c, def));
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        let mut map: IndexMap<String, (String, Chunk, bool)> = IndexMap::new();
        for (c, def) in chain.into_iter().rev() {
            for (name, chunk) in &def.prop_defaults {
                let uninit = def.uninit_props.contains_key(name);
                map.insert(name.clone(), (c.clone(), chunk.clone(), uninit));
            }
        }
        Some(map.into_iter().collect())
    }

    /// The instance properties `class` declares, parent's first, in the slot
    /// order a new instance lays them out in. Empty for an unknown class
    /// (`stdClass`), whose every property is dynamic.
    fn declared_prop_order(&self, class: &str) -> Vec<String> {
        let mut chain: Vec<&ClassDef> = Vec::new();
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let Some(def) = self.classes.get(&c) else {
                break;
            };
            chain.push(def);
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        let mut order: IndexMap<String, ()> = IndexMap::new();
        for def in chain.into_iter().rev() {
            for (name, _) in &def.prop_defaults {
                order.insert(name.clone(), ());
            }
        }
        order.into_keys().collect()
    }

    /// `(declaring class, type)` when `name`, as the nearest declaration along
    /// `class`'s chain has it, is a typed property with no default — one that is
    /// uninitialized whenever the instance does not hold it.
    fn uninit_prop_decl(&self, class: &str, name: &str) -> Option<(String, String)> {
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let def = self.classes.get(&c)?;
            if def.prop_defaults.iter().any(|(n, _)| n == name) {
                return def
                    .uninit_props
                    .get(name)
                    .map(|ty| (self.class_display_name(&c), ty.clone()));
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        None
    }

    /// The `Error` a read of `$recv->name` raises when the property is typed and
    /// still uninitialized (never written, or `unset()`), or `None`.
    pub fn uninit_read_error(&self, recv: &Value, name: &str) -> Option<String> {
        let Some(PhpObj::Object { class, props }) = self.as_array(recv) else {
            return None;
        };
        if props.contains_key(name) {
            return None;
        }
        let (declaring, _) = self.uninit_prop_decl(class, name)?;
        Some(format!(
            "Typed property {declaring}::${name} must not be accessed before initialization"
        ))
    }

    /// Record an `unset()` of `$recv->name`, whether or not it was set, when it
    /// is a typed property with no default — see `unset_typed`.
    pub fn note_unset(&mut self, recv: &Value, name: &str) {
        if let (Value::Obj(id), Some(PhpObj::Object { class, .. })) = (recv, self.as_array(recv)) {
            if self.uninit_prop_decl(class, name).is_some() {
                self.unset_typed
                    .entry(*id)
                    .or_default()
                    .insert(name.to_string());
            }
        }
    }

    /// [`Self::uninit_read_error`] for a class with `__get`: the magic method
    /// answers for an uninitialized typed property only once `unset()` has
    /// removed it, so a never-written one is still the `Error`.
    pub fn uninit_magic_blocked(&self, recv: &Value, name: &str) -> Option<String> {
        let Value::Obj(id) = recv else { return None };
        if self.unset_typed.get(id).is_some_and(|s| s.contains(name)) {
            return None;
        }
        self.uninit_read_error(recv, name)
    }

    /// The uninitialized typed properties of an object, for `var_dump`: each as
    /// `(position, name, type)`, where `position` is the index into the object's
    /// property list the entry prints BEFORE. Declared properties occupy fixed
    /// slots ahead of every dynamic one, so an uninitialized slot prints where
    /// its declaration puts it.
    pub fn uninit_slots(&self, recv: &Value) -> Vec<(usize, String, String)> {
        let Some(PhpObj::Object { class, props }) = self.as_array(recv) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut present = 0;
        for name in self.declared_prop_order(class) {
            if props.contains_key(&name) {
                present += 1;
            } else if let Some((_, ty)) = self.uninit_prop_decl(class, &name) {
                out.push((present, name, ty));
            }
        }
        out
    }

    /// Store a property that may not be in the object yet. A DECLARED property
    /// coming back after `unset()`, or taking its first write while
    /// uninitialized, goes back into its declared slot rather than after the
    /// dynamic ones — `foreach` and `var_dump` list it where the class declared
    /// it, as the reference does. Anything else is appended.
    fn obj_put_prop(&mut self, recv: &Value, name: &str, val: Value) {
        let at = match self.as_array(recv) {
            Some(PhpObj::Object { props, .. }) if props.contains_key(name) => None,
            Some(PhpObj::Object { class, props }) => {
                let order = self.declared_prop_order(class);
                order.iter().position(|n| n == name).map(|mine| {
                    props
                        .keys()
                        .filter(|k| order[..mine].iter().any(|d| d == *k))
                        .count()
                })
            }
            _ => return,
        };
        if let Some(PhpObj::Object { props, .. }) = self.as_array_mut(recv) {
            match at {
                Some(i) => {
                    props.shift_insert(i, name.to_string(), val);
                }
                None => {
                    props.insert(name.to_string(), val);
                }
            }
        }
    }

    /// Resolve `Class::$name` to its storage key and initializer chunk, walking
    /// the parent chain to the class that actually declares the static property.
    /// The key is `"declaringclass::name"` so a subclass shares the parent's cell.
    /// The class (lowercased) that declares static property `name`, seen from
    /// `class`.
    pub fn resolve_static_owner(&self, class: &str, name: &str) -> Option<String> {
        self.resolve_static_key(class, name)
            .and_then(|(key, _)| key.split_once("::").map(|(c, _)| c.to_string()))
    }

    fn resolve_static_key(&self, class: &str, name: &str) -> Option<(String, Chunk)> {
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let def = self.classes.get(&c)?;
            if let Some((_, chunk)) = def.static_prop_defaults.iter().find(|(n, _)| n == name) {
                return Some((format!("{c}::{name}"), chunk.clone()));
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        None
    }

    /// The stored value of a static property, by storage key (`None` = not yet
    /// initialized).
    fn get_static_stored(&self, key: &str) -> Option<Value> {
        self.static_props.get(key).cloned()
    }

    /// Store a static property's value by storage key.
    fn set_static_stored(&mut self, key: &str, val: Value) {
        self.static_props.insert(key.to_string(), val);
    }

    /// Bind a function-local `$name` to its persistent `static` slot. On first
    /// encounter the cell is created with `init`; later calls reuse it, so the
    /// value survives across calls. The current scope's name is aliased to the
    /// cell (like a reference), so ordinary reads/writes of `$name` in the body
    /// hit the persistent storage.
    pub fn bind_static_local(&mut self, name: &str, slot_key: &str, init: Value) {
        let slot = match self.static_slots.get(slot_key) {
            Some(&s) => s,
            None => {
                self.ref_cells.push(init);
                let s = self.ref_cells.len() - 1;
                self.static_slots.insert(slot_key.to_string(), s);
                s
            }
        };
        let idx = self.scope_idx(name);
        if let Some(scope) = self.scopes.get_mut(idx) {
            let i = scope.vars.ensure_slot(name);
            scope.vars.put(i, Slot::Ref(slot));
        }
    }

    // ── enums (PHP 8.1) ──────────────────────────────────────────────────────

    /// Whether `class` is an `enum` (case-insensitive).
    /// A class name as it was DECLARED, from any spelling of it. Class names are
    /// case-insensitive and keyed lowercase, so a diagnostic that echoes back the
    /// caller's spelling reads wrong; the reference always prints the declaration's.
    /// An undeclared name is handed back unchanged.
    pub fn declared_class_name(&self, class: &str) -> String {
        self.classes
            .get(&class.to_ascii_lowercase())
            .map_or_else(|| class.to_string(), |d| d.name.clone())
    }

    pub fn is_enum_class(&self, class: &str) -> bool {
        self.classes
            .get(&class.to_ascii_lowercase())
            .map(|d| d.is_enum)
            .unwrap_or(false)
    }

    /// The declared case's optional backing-value chunk (`Some(chunk_opt)` when the
    /// enum declares a case of this exact name, else `None`).
    fn enum_case_chunk(&self, class: &str, case: &str) -> Option<Option<Chunk>> {
        let d = self.classes.get(&class.to_ascii_lowercase())?;
        d.enum_cases
            .iter()
            .find(|(n, _)| n == case)
            .map(|(_, c)| c.clone())
    }

    /// The enum's case names in declaration order.
    fn enum_case_names(&self, class: &str) -> Vec<String> {
        self.classes
            .get(&class.to_ascii_lowercase())
            .map(|d| d.enum_cases.iter().map(|(n, _)| n.clone()).collect())
            .unwrap_or_default()
    }

    fn enum_case_cached(&self, key: &str) -> Option<Value> {
        self.enum_case_cache.get(key).cloned()
    }

    fn enum_case_store(&mut self, key: &str, v: Value) {
        self.enum_case_cache.insert(key.to_string(), v);
    }

    /// Allocate an enum-case instance with its `name` (and, for a backed enum,
    /// `value`) properties. `class` keeps its source casing for `get_class`.
    fn new_enum_object(&mut self, class: &str, name: &str, value: Option<Value>) -> Value {
        let mut props: IndexMap<String, Value> = IndexMap::new();
        props.insert("name".to_string(), Value::str(name.to_string()));
        if let Some(v) = value {
            props.insert("value".to_string(), v);
        }
        self.objs.push(PhpObj::Object {
            class: class.to_string(),
            props,
        });
        Value::Obj((self.objs.len() - 1) as u32)
    }

    /// The initializer chunk for `Class::name`, walking the parent chain and
    /// then the interfaces.
    ///
    /// The interfaces are not optional. A `const` declared in an interface is
    /// inherited by every class that implements it and by every interface that
    /// extends it, so `interface I { const K = 5; } class C implements I {}`
    /// answers `C::K` — and `self::K` and `static::K` from inside `C` are the
    /// same lookup. Walking only `parent` reported `Undefined constant C::K`
    /// for all three.
    ///
    /// Classes first, interfaces after: a class constant shadows the interface
    /// one of the same name (PHP 8.1 permits the override), and searching the
    /// whole parent chain before any interface is what makes that hold at every
    /// depth. The visited set and bound guard a malformed cycle, as in
    /// [`Host::class_is_a`].
    /// The `Error` a `Class::NAME` read raises when the calling scope cannot
    /// reach a `private`/`protected` constant, worded as `zend_get_class_constant_ex`
    /// (Zend/zend_constants.c) words it: the visibility and the class NAMED at
    /// the access site. Only the class chain can declare a non-public constant
    /// (an interface constant is always public), so only it is walked.
    fn const_access_denied(&self, class: &str, name: &str) -> Option<String> {
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let def = self.classes.get(&c)?;
            if def.consts.iter().any(|(n, _)| n == name) {
                let vis = *def.const_vis.get(name)?;
                if self.visibility_allows(vis, &c) {
                    return None;
                }
                let vname = match vis {
                    Visibility::Private => "private",
                    Visibility::Protected => "protected",
                    Visibility::Public => return None,
                };
                let shown = self.class_display_name(&class.to_ascii_lowercase());
                return Some(format!("Cannot access {vname} constant {shown}::{name}"));
            }
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        None
    }

    fn resolve_const_chunk(&self, class: &str, name: &str) -> Option<(String, Chunk)> {
        let mut ifaces: Vec<String> = Vec::new();
        let mut cur = Some(class.to_ascii_lowercase());
        while let Some(c) = cur {
            let Some(def) = self.classes.get(&c) else {
                break;
            };
            if let Some((_, chunk)) = def.consts.iter().find(|(n, _)| n == name) {
                return Some((c, chunk.clone()));
            }
            ifaces.extend(def.interfaces.iter().map(|i| i.to_ascii_lowercase()));
            cur = def.parent.as_ref().map(|p| p.to_ascii_lowercase());
        }
        let mut seen: Vec<String> = Vec::new();
        while let Some(i) = ifaces.pop() {
            if seen.contains(&i) || seen.len() > 1000 {
                continue;
            }
            seen.push(i.clone());
            if let Some(def) = self.classes.get(&i) {
                if let Some((_, chunk)) = def.consts.iter().find(|(n, _)| n == name) {
                    return Some((i.clone(), chunk.clone()));
                }
                ifaces.extend(def.interfaces.iter().map(|x| x.to_ascii_lowercase()));
                if let Some(p) = &def.parent {
                    ifaces.push(p.to_ascii_lowercase());
                }
            }
        }
        None
    }

    fn as_array_mut(&mut self, v: &Value) -> Option<&mut PhpObj> {
        match v {
            Value::Obj(h) => self.objs.get_mut(*h as usize),
            _ => None,
        }
    }

    fn as_array(&self, v: &Value) -> Option<&PhpObj> {
        match v {
            Value::Obj(h) => self.objs.get(*h as usize),
            _ => None,
        }
    }

    /// Normalize a value used as an array key, matching PHP: integer-valued
    /// strings and floats become int keys, bools/null fold to int/"".
    fn norm_key(&self, key: &Value) -> ArrayKey {
        match key {
            Value::Int(n) => ArrayKey::Int(*n),
            Value::Bool(b) => ArrayKey::Int(*b as i64),
            // `as i64` saturates where PHP wraps, so `$a[1e19]` landed on
            // `PHP_INT_MAX` instead of -8446744073709551616.
            Value::Float(f) => ArrayKey::Int(dval_to_lval(*f)),
            Value::Undef => ArrayKey::Str(String::new()),
            Value::Str(s) => match canonical_int_key(s) {
                Some(n) => ArrayKey::Int(n),
                None => ArrayKey::Str(String::clone(s)),
            },
            Value::Obj(_) if self.is_stream(key) => {
                ArrayKey::Int(self.resource_info(key).map_or(0, |(id, _)| id))
            }
            Value::Obj(_) => ArrayKey::Str("Array".into()),
            _ => ArrayKey::Str(self.to_str(key)),
        }
    }

    /// The reference slot the entry `arr[k]` holds, if that element is a reference.
    fn entry_ref_slot(&self, arr: &Value, k: &ArrayKey) -> Option<usize> {
        let Some(PhpObj::Array { entries, .. }) = self.as_array(arr) else {
            return None;
        };
        self.ref_slot_of_value(entries.get(k)?)
    }

    /// The class of `recv` when it is an object implementing `ArrayAccess`, so
    /// `$o[k]` must route through `offsetGet`/`offsetSet`/`offsetExists`/
    /// `offsetUnset` instead of touching an array.
    ///
    /// Returned as a name rather than a bool because every caller immediately
    /// needs it to dispatch the method, and that dispatch runs PHP — it cannot
    /// happen while the host is borrowed.
    pub fn array_access_class(&self, recv: &Value) -> Option<String> {
        let class = self.object_class(recv)?;
        self.class_is_a_pub(&class, "ArrayAccess").then_some(class)
    }

    /// `$arr[key]` read. Also indexes strings (single-character substring).
    pub fn index_get(&self, recv: &Value, key: &Value) -> Value {
        if let Some(PhpObj::Array { entries, .. }) = self.as_array(recv) {
            let k = self.norm_key(key);
            let raw = entries.get(&k).cloned().unwrap_or(Value::Undef);
            return self.deref(raw);
        }
        if let Value::Str(s) = recv {
            // PHP string offsets are byte-indexed and accept negatives (`$s[-1]`
            // is the last byte). An out-of-range offset yields `Undef`, so
            // `isset($s[i])` is false there (a plain read still echoes as "").
            let bytes = s.as_bytes();
            let len = bytes.len() as i64;
            let StrOffset::At(mut i, _) = classify_string_offset(key) else {
                return Value::Undef;
            };
            if i < 0 {
                i += len;
            }
            if i >= 0 && i < len {
                let b = i as usize;
                return Value::str(String::from_utf8_lossy(&bytes[b..b + 1]).into_owned());
            }
        }
        Value::Undef
    }

    /// `$arr[key]` read in a *value* context, where a miss is a mistake rather
    /// than a question. Same result as `index_get`, plus PHP's diagnostic:
    /// `Undefined array key K` for a missing element, `Uninitialized string
    /// offset N` past the end of a string, and `Trying to access array offset on
    /// <type>` when the receiver is not subscriptable at all.
    pub fn index_get_warn(&mut self, recv: &Value, key: &Value) -> Value {
        self.diagnose_array_offset(recv, key);
        if let Some(PhpObj::Array { entries, .. }) = self.as_array(recv) {
            let k = self.norm_key(key);
            match entries.get(&k).cloned() {
                Some(v) => return self.deref(v),
                None => {
                    let k = self.diag_key(key);
                    self.warn(format_args!("Undefined array key {k}"));
                    return Value::Undef;
                }
            }
        }
        if let Value::Str(s) = recv {
            let len = s.len() as i64;
            // A key that is no offset at all was rejected by the caller (see
            // `classify_string_offset`); reaching here it is always `At`.
            let StrOffset::At(off, cast) = classify_string_offset(key) else {
                return Value::Undef;
            };
            if cast {
                self.warn("String offset cast occurred");
            }
            let i = if off < 0 { off + len } else { off };
            if i >= 0 && i < len {
                return self.index_get(recv, key);
            }
            self.warn(format_args!("Uninitialized string offset {off}"));
            return Value::Undef;
        }
        // A closure/generator/resource handle is not an array either, but PHP
        // raises an Error for those rather than a warning; only the scalars and
        // null reach this warning.
        if !matches!(recv, Value::Obj(_)) {
            let t = self.diag_offset_type(recv);
            self.warn(format_args!("Trying to access array offset on {t}"));
        }
        Value::Undef
    }

    /// `$name` read in a value context — `get_var` plus `Undefined variable $x`
    /// when the name is not bound. Compiler temporaries (which are prefixed with
    /// `@`, outside the PHP identifier space) never warn: they are not the user's
    /// variables and are always written before they are read.
    pub fn get_var_warn(&mut self, name: &str) -> Value {
        let idx = self.scope_idx(name);
        if let Some(scope) = self.scopes.get(idx) {
            let s = scope.vars.get(name).clone();
            if !matches!(s, Slot::Unset) {
                return self.read_slot(&s);
            }
        }
        if !name.starts_with('@') {
            self.warn(format_args!("Undefined variable ${name}"));
        }
        Value::Undef
    }

    /// `$obj->name` read in a value context — `prop_get` plus PHP's diagnostic:
    /// `Undefined property: C::$p` when the instance has no such property, and
    /// `Attempt to read property "p" on <type>` when the receiver is not an
    /// object at all.
    pub fn prop_get_warn(&mut self, recv: &Value, name: &str) -> Value {
        match self.as_array(recv) {
            Some(PhpObj::Object { class, props }) => match props.get(name).cloned() {
                Some(v) => self.deref(v),
                None => {
                    let class = display_class(class).to_string();
                    self.warn(format_args!("Undefined property: {class}::${name}"));
                    Value::Undef
                }
            },
            // An array handle reads as a property-less value; PHP reports the
            // same "read property on array" as for any other non-object.
            _ => {
                let t = self.diag_type(recv);
                self.warn(format_args!("Attempt to read property \"{name}\" on {t}"));
                Value::Undef
            }
        }
    }

    /// `$var[key] = val` on the named scope variable, auto-vivifying an array.
    ///
    /// A variable already holding a STRING is not auto-vivified: the write edits
    /// that string in place. See `Self::string_offset_set` — before it existed
    /// every `$s[0] = "x"` silently replaced the string with a one-element array.
    pub fn index_set_var(&mut self, name: &str, key: &Value, val: Value) -> Result<(), String> {
        if let Value::Str(cur) = self.get_var(name) {
            let updated = self.string_offset_set(&cur, key, &val)?;
            if let Some(updated) = updated {
                self.set_var(name, Value::str(updated));
            }
            return Ok(());
        }
        let arr = self.ensure_array_var(name);
        self.arr_set_key(&arr, key, val);
        Ok(())
    }

    /// `$s[offset] = $v` where `$s` is a string. Returns the new string, or
    /// `None` when the write was refused with a warning and `$s` is unchanged.
    ///
    /// Ported from `zend_assign_to_string_offset`. The rules, each measured
    /// against `php 8.5.9`:
    ///
    /// | form | outcome |
    /// |---|---|
    /// | `$s="abc"; $s[1]="Z"` | `"aZc"` |
    /// | `$s="abc"; $s[5]="Z"` | `"abc  Z"` — the gap is padded with SPACES |
    /// | `$s="abc"; $s[-1]="Z"` | `"abZ"` — negative counts from the end |
    /// | `$s="abc"; $s[-10]="Z"` | Warning `Illegal string offset -10`, unchanged |
    /// | `$s="abc"; $s[1]="XY"` | Warning `Only the first byte…`, `"aXc"` |
    /// | `$s="abc"; $s[1]=""` | Error `Cannot assign an empty string to a string offset` |
    /// | `$s="abc"; $s["x"]="Z"` | TypeError `Cannot access offset of type string on string` |
    /// | `$s="abc"; $s[1.7]="Z"` | Warning `String offset cast occurred`, `"aZc"` |
    fn string_offset_set(
        &mut self,
        cur: &str,
        key: &Value,
        val: &Value,
    ) -> Result<Option<String>, String> {
        // Only an int-ish offset addresses a string. A non-numeric string key is
        // a TypeError; a float/bool/null is accepted with a cast warning.
        let off = match key {
            Value::Int(n) => *n,
            Value::Str(k) => match parse_php_number_full(k) {
                Some(Value::Int(n)) => n,
                _ => {
                    return Err(crate::builtins::throws_bare(
                        "TypeError",
                        format!(
                            "Cannot access offset of type {} on string",
                            self.type_name_for_error(key)
                        ),
                    ))
                }
            },
            Value::Float(_) | Value::Bool(_) | Value::Undef => {
                self.warn("String offset cast occurred");
                self.to_number(key).to_int()
            }
            other => {
                return Err(crate::builtins::throws_bare(
                    "TypeError",
                    format!(
                        "Cannot access offset of type {} on string",
                        self.type_name_for_error(other)
                    ),
                ))
            }
        };

        let replacement = self.to_str(val);
        if replacement.is_empty() {
            return Err(crate::builtins::throws_bare(
                "Error",
                "Cannot assign an empty string to a string offset",
            ));
        }
        if replacement.len() > 1 {
            self.warn("Only the first byte will be assigned to the string offset");
        }
        let byte = replacement.as_bytes()[0];

        let len = cur.len() as i64;
        let idx = if off < 0 {
            let from_end = len + off;
            if from_end < 0 {
                self.warn(format_args!("Illegal string offset {off}"));
                return Ok(None);
            }
            from_end as usize
        } else {
            off as usize
        };

        let mut bytes = cur.as_bytes().to_vec();
        if idx >= bytes.len() {
            bytes.resize(idx + 1, b' ');
        }
        bytes[idx] = byte;
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// `$var[] = val` append on the named scope variable, auto-vivifying.
    ///
    /// `Err` carries [`NEXT_ELEMENT_OCCUPIED`] when the array already holds an
    /// element at `PHP_INT_MAX` — the reference refuses the write rather than
    /// picking some other key, and the refusal is a catchable `Error`.
    pub fn append_var(&mut self, name: &str, val: Value) -> Result<(), String> {
        let arr = self.ensure_array_var(name);
        if self.append_slot_taken(&arr) {
            return Err(NEXT_ELEMENT_OCCUPIED.to_string());
        }
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(&arr)
        {
            let k = ArrayKey::Int(*next_index);
            *next_index = next_index.saturating_add(1);
            entries.insert(k, val);
        }
        Ok(())
    }

    /// Return the array handle held by `$name`, creating an empty array (and
    /// storing it back) if the variable is unset or not an array.
    fn ensure_array_var(&mut self, name: &str) -> Value {
        let cur = self.get_var(name);
        if matches!(self.as_array(&cur), Some(PhpObj::Array { .. })) {
            return cur;
        }
        let arr = self.new_array();
        self.set_var(name, arr.clone());
        arr
    }

    /// Descend the array handle `arr` along `keys`, auto-vivifying an array at each
    /// step, and return the handle of the array the final segment lands in. A slot
    /// that is unset — or holds a non-array — is (re)created as an empty array,
    /// matching PHP's auto-vivification of nested lvalues.
    fn ensure_path_from(&mut self, mut arr: Value, keys: &[Value]) -> Value {
        for key in keys {
            let k = self.norm_key(key);
            let child = match self.as_array(&arr) {
                // A referenced element descends into whatever its cell holds, so
                // `$r = &$a['x']; $a['x']['y'] = 1;` writes through the alias.
                Some(PhpObj::Array { entries, .. }) => {
                    entries.get(&k).cloned().map(|v| self.deref(v))
                }
                _ => None,
            };
            arr = match child {
                Some(v) if self.is_array(&v) => v,
                _ => {
                    let new = self.new_array();
                    self.arr_set_key(&arr, key, new.clone());
                    new
                }
            };
        }
        arr
    }

    /// Like `ensure_path_from`, but rooted at the scope variable `$name`
    /// (auto-vivifying the variable itself into an array).
    fn ensure_path_array(&mut self, name: &str, keys: &[Value]) -> Value {
        let root = self.ensure_array_var(name);
        self.ensure_path_from(root, keys)
    }

    /// `$name[k1]..[kN] = val` — set the deepest element along a key path,
    /// auto-vivifying the intermediate arrays (`N >= 1`).
    pub fn index_set_path(&mut self, name: &str, keys: &[Value], val: Value) {
        let Some((last, inter)) = keys.split_last() else {
            return;
        };
        let arr = self.ensure_path_array(name, inter);
        self.arr_set_key(&arr, last, val);
    }

    /// `$name[k1]..[kM][] = val` — append into the array reached along `keys`,
    /// auto-vivifying (`M >= 0`, so `$a[] = v` is the empty-path case).
    pub fn append_path(&mut self, name: &str, keys: &[Value], val: Value) {
        let arr = self.ensure_path_array(name, keys);
        self.arr_push_auto(&arr, val);
    }

    /// Read `$name[k1]..[kN]` without mutating (for read-modify-write compound
    /// assignment and `++`/`--` on an element). Missing slots read as `Undef`.
    pub fn index_get_path(&self, name: &str, keys: &[Value]) -> Value {
        let mut cur = self.get_var(name);
        for key in keys {
            cur = self.index_get(&cur, key);
        }
        cur
    }

    /// `index_get_path` for the read half of a read-modify-write — `$a[k] += 1`,
    /// `$a[k]++` — which diagnoses every step of the path.
    ///
    /// PHP fetches an RW path in write mode: an unset container is reported and
    /// then *auto-vivified* into an empty array, so the next segment reports its
    /// own missing key rather than "array offset on null". That is what makes
    /// `$a['p']['q'] += 5` on a fresh `$a` print `Undefined variable $a`,
    /// `Undefined array key "p"`, `Undefined array key "q"` — three misses, not a
    /// miss followed by two null-offset complaints, which is what a plain read of
    /// the same path prints.
    pub fn index_get_path_warn(&mut self, name: &str, keys: &[Value]) -> Value {
        let mut cur = self.get_var_warn(name);
        for key in keys {
            if matches!(cur, Value::Undef) {
                // The write-mode fetch put an empty array here; the key is still
                // missing from it.
                let k = self.diag_key(key);
                self.warn(format_args!("Undefined array key {k}"));
                continue;
            }
            cur = self.index_get_warn(&cur, key);
        }
        cur
    }

    /// Append a fresh empty array as a new element of `$name[k1]..[kN]` and return
    /// its handle — the pivot for a mid-path append (`$a[][k] = v`): the caller
    /// keeps writing through the returned child handle, so each `[]` in the chain
    /// appends exactly one new element the way PHP does.
    pub fn path_append_child(&mut self, name: &str, keys: &[Value]) -> Value {
        let arr = self.ensure_path_array(name, keys);
        let new = self.new_array();
        self.arr_push_auto(&arr, new.clone());
        new
    }

    /// Whether an append to `arr` has nowhere left to go — the next integer key
    /// is already taken, which only happens once `next_index` has saturated at
    /// `PHP_INT_MAX` because some element was written under that exact key.
    ///
    /// The reference detects this in `_zend_hash_index_add_or_update_i`: an
    /// append is an ADD at `nNextFreeElement`, and an ADD onto an existing key
    /// fails rather than overwriting. `false` for a non-array, which leaves the
    /// caller's own type handling in charge.
    pub fn append_slot_taken(&self, arr: &Value) -> bool {
        matches!(
            self.as_array(arr),
            Some(PhpObj::Array { entries, next_index })
                if entries.contains_key(&ArrayKey::Int(*next_index))
        )
    }

    /// Append `v` under the next integer key of the array `arr` (a handle).
    ///
    /// Saturating: once `next_index` reaches `PHP_INT_MAX` this overwrites that
    /// key rather than overflowing. Callers reachable from `$a[] =` must consult
    /// [`PhpHost::append_slot_taken`] first and raise [`NEXT_ELEMENT_OCCUPIED`];
    /// the many stdlib callers that build a FRESH array cannot reach the
    /// saturated state and so do not check.
    pub fn arr_push_auto(&mut self, arr: &Value, v: Value) {
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(arr)
        {
            let k = ArrayKey::Int(*next_index);
            *next_index = next_index.saturating_add(1);
            entries.insert(k, v);
        }
    }

    /// Insert `v` under `key` in the array `arr` (a handle). An element that a `&`
    /// binding has turned into a reference is written *through* — the alias sees
    /// the new value and the slot stays a reference — instead of being replaced.
    pub fn arr_set_key(&mut self, arr: &Value, key: &Value, v: Value) {
        self.diagnose_array_offset(arr, key);
        let k = self.norm_key(key);
        if let Some(slot) = self.entry_ref_slot(arr, &k) {
            self.ref_cell_set(slot, v);
            return;
        }
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(arr)
        {
            if let ArrayKey::Int(n) = k {
                if n >= *next_index {
                    *next_index = n.saturating_add(1);
                }
            }
            entries.insert(k, v);
        }
    }

    /// Replace an array handle's entries with a re-indexed (`0..n`) value list,
    /// in place — the mutation is visible through every variable holding the same
    /// handle (`sort`/`rsort`). No-op if `arr` is not an array.
    /// Replace every entry of `arr` with `src`'s, keys included — how a
    /// by-reference OUT array is written through a handle the caller already
    /// held. A no-op unless both are arrays.
    pub fn arr_replace_all(&mut self, arr: &Value, src: &Value) {
        let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array(src)
        else {
            return;
        };
        let (entries, next_index) = (entries.clone(), *next_index);
        if let Some(PhpObj::Array {
            entries: dst,
            next_index: dst_next,
        }) = self.as_array_mut(arr)
        {
            *dst = entries;
            *dst_next = next_index;
        }
    }

    pub fn arr_set_reindexed(&mut self, arr: &Value, vals: Vec<Value>) {
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(arr)
        {
            entries.clear();
            *next_index = 0;
            for v in vals {
                let k = ArrayKey::Int(*next_index);
                *next_index = next_index.saturating_add(1);
                entries.insert(k, v);
            }
        }
    }

    /// Replace an array handle's entries with an explicit ordered `(key, value)`
    /// list, preserving keys, in place (`asort`/`ksort`).
    pub fn arr_set_pairs(&mut self, arr: &Value, pairs: Vec<(Value, Value)>) {
        // Normalize keys under `&self` first, then take the `&mut self` borrow.
        let normed: Vec<(ArrayKey, Value)> = pairs
            .into_iter()
            .map(|(k, v)| (self.norm_key(&k), v))
            .collect();
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(arr)
        {
            entries.clear();
            *next_index = 0;
            for (k, v) in normed {
                if let ArrayKey::Int(n) = k {
                    if n >= *next_index {
                        *next_index = n.saturating_add(1);
                    }
                }
                entries.insert(k, v);
            }
        }
    }

    /// Rebuild an array handle's entries from an ordered `(key, value)` list,
    /// renumbering integer keys `0..` while preserving string keys — the shared
    /// core of `array_shift`/`array_unshift`/`array_splice`. No-op if `arr` is not
    /// an array.
    fn arr_rebuild_reindexed(&mut self, arr: &Value, pairs: Vec<(Value, Value)>) {
        // Normalize keys under `&self` first, then take the `&mut self` borrow.
        let normed: Vec<(ArrayKey, Value)> = pairs
            .into_iter()
            .map(|(k, v)| (self.norm_key(&k), v))
            .collect();
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(arr)
        {
            entries.clear();
            *next_index = 0;
            for (k, v) in normed {
                match k {
                    // Integer keys are renumbered sequentially; string keys stay.
                    ArrayKey::Int(_) => {
                        let nk = ArrayKey::Int(*next_index);
                        *next_index = next_index.saturating_add(1);
                        entries.insert(nk, v);
                    }
                    ArrayKey::Str(_) => {
                        entries.insert(k, v);
                    }
                }
            }
        }
    }

    /// Remove and return the last element of `$var` (`array_pop`). Remaining keys
    /// are left untouched. PHP resets the next-append index only when the popped
    /// element held the top of the current append run (its key was `next_index-1`);
    /// a sparse/gapped array (e.g. `[5=>a, 10=>b]`) keeps its `next_index`.
    pub fn arr_pop_var(&mut self, name: &str) -> Value {
        let arr = self.get_var(name);
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(&arr)
        {
            let before = *next_index;
            let popped = entries.pop();
            if let Some((ArrayKey::Int(n), _)) = &popped {
                if *n == before - 1 {
                    *next_index = *n;
                }
            }
            return popped.map(|(_, v)| v).unwrap_or(Value::Undef);
        }
        Value::Undef
    }

    /// Remove and return the first element of `$var` (`array_shift`), reindexing
    /// integer keys from `0` while preserving string keys. Returns null if the
    /// variable is unset/empty or not an array.
    pub fn arr_shift_var(&mut self, name: &str) -> Value {
        let arr = self.get_var(name);
        let Some(mut pairs) = self.array_pairs(&arr) else {
            return Value::Undef;
        };
        if pairs.is_empty() {
            return Value::Undef;
        }
        let (_, first) = pairs.remove(0);
        self.arr_rebuild_reindexed(&arr, pairs);
        first
    }

    /// Append each value to `$var` (`array_push`), auto-vivifying an array;
    /// returns the new element count.
    /// `Err` carries [`NEXT_ELEMENT_OCCUPIED`] once the array's next integer key
    /// is taken — `array_push` refuses the write for the same reason `$a[] =`
    /// does, and the refusal is a catchable `Error`.
    pub fn arr_push_var(&mut self, name: &str, vals: Vec<Value>) -> Result<Value, String> {
        let arr = self.ensure_array_var(name);
        for v in vals {
            if self.append_slot_taken(&arr) {
                return Err(NEXT_ELEMENT_OCCUPIED.to_string());
            }
            self.arr_push_auto(&arr, v);
        }
        Ok(Value::int(self.array_len(&arr)))
    }

    /// Prepend `vals` to `$var` (`array_unshift`) as a fresh `0`-based run,
    /// reindexing the existing integer keys after them (string keys preserved);
    /// returns the new element count.
    pub fn arr_unshift_var(&mut self, name: &str, vals: Vec<Value>) -> Value {
        let arr = self.ensure_array_var(name);
        let existing = self.array_pairs(&arr).unwrap_or_default();
        let mut combined: Vec<(Value, Value)> = Vec::with_capacity(vals.len() + existing.len());
        // A placeholder integer key marks each new value for renumbering; the
        // rebuild renumbers all integer keys, so the placeholder value is unused.
        for v in vals {
            combined.push((Value::int(0), v));
        }
        combined.extend(existing);
        self.arr_rebuild_reindexed(&arr, combined);
        Value::int(self.array_len(&arr))
    }

    /// `array_splice($var, offset, length?, replacement?)` — remove `length`
    /// elements at `offset` (negatives count from the end; omitted length runs to
    /// the end) and splice `replacement` (an array or a single value) in their
    /// place, reindexing integer keys. Returns a new array of the removed
    /// elements.
    pub fn arr_splice_var(&mut self, name: &str, args: &[Value]) -> Value {
        let arr = self.ensure_array_var(name);
        let pairs = self.array_pairs(&arr).unwrap_or_default();
        let n = pairs.len() as i64;
        let mut off = args.first().map(|v| v.to_int()).unwrap_or(0);
        if off < 0 {
            off = (n + off).max(0);
        }
        let off = off.min(n).max(0) as usize;
        let len = match args.get(1) {
            Some(v) if !matches!(v, Value::Undef) => {
                let l = v.to_int();
                if l < 0 {
                    (n - off as i64 + l).max(0) as usize
                } else {
                    (l as usize).min(pairs.len() - off)
                }
            }
            _ => pairs.len() - off,
        };
        let end = (off + len).min(pairs.len());
        // The replacement flattens to a value list (an array's keys are dropped).
        let replacement: Vec<Value> = match args.get(2) {
            Some(v) if self.is_array(v) => self
                .array_pairs(v)
                .unwrap_or_default()
                .into_iter()
                .map(|(_, x)| x)
                .collect(),
            Some(v) if !matches!(v, Value::Undef) => vec![v.clone()],
            _ => Vec::new(),
        };
        let removed: Vec<(Value, Value)> = pairs[off..end].to_vec();
        // Rebuild the source: kept prefix, replacement run, kept suffix.
        let mut rebuilt: Vec<(Value, Value)> = Vec::with_capacity(pairs.len());
        rebuilt.extend(pairs[..off].iter().cloned());
        for v in replacement {
            rebuilt.push((Value::int(0), v));
        }
        rebuilt.extend(pairs[end..].iter().cloned());
        self.arr_rebuild_reindexed(&arr, rebuilt);
        let out = self.new_array();
        self.arr_rebuild_reindexed(&out, removed);
        out
    }

    pub fn array_keys(&mut self, recv: &Value) -> Value {
        let keys: Vec<Value> = match self.as_array(recv) {
            Some(PhpObj::Array { entries, .. }) => entries.keys().map(|k| k.to_value()).collect(),
            _ => Vec::new(),
        };
        let arr = self.new_array();
        if let Some(PhpObj::Array {
            entries,
            next_index,
        }) = self.as_array_mut(&arr)
        {
            for (i, k) in keys.into_iter().enumerate() {
                entries.insert(ArrayKey::Int(i as i64), k);
                *next_index = (i + 1) as i64;
            }
        }
        arr
    }

    pub fn array_len(&self, recv: &Value) -> i64 {
        match self.as_array(recv) {
            Some(PhpObj::Array { entries, .. }) => entries.len() as i64,
            _ => 0,
        }
    }

    /// The array's `(key, value)` pairs, cloned (for `print_r`/`implode`/etc.).
    pub fn array_pairs(&self, recv: &Value) -> Option<Vec<(Value, Value)>> {
        match self.as_array(recv) {
            Some(PhpObj::Array { entries, .. }) => {
                let raw: Vec<(Value, Value)> = entries
                    .iter()
                    .map(|(k, v)| (k.to_value(), v.clone()))
                    .collect();
                Some(raw.into_iter().map(|(k, v)| (k, self.deref(v))).collect())
            }
            _ => None,
        }
    }

    /// The array's `(key, value, is_reference)` triples, values still raw. Only
    /// `var_dump`, which prints a `&` before a referenced element, needs to see
    /// which slots are references; every other reader wants `array_pairs`.
    pub fn array_pairs_marked(&self, recv: &Value) -> Option<Vec<(Value, Value, bool)>> {
        let Some(PhpObj::Array { entries, .. }) = self.as_array(recv) else {
            return None;
        };
        let raw: Vec<(Value, Value)> = entries
            .iter()
            .map(|(k, v)| (k.to_value(), v.clone()))
            .collect();
        Some(
            raw.into_iter()
                .map(|(k, v)| {
                    let is_ref = self.ref_slot_of_value(&v).is_some();
                    (k, self.deref(v), is_ref)
                })
                .collect(),
        )
    }

    pub fn is_array(&self, v: &Value) -> bool {
        matches!(self.as_array(v), Some(PhpObj::Array { .. }))
    }

    // ── value coercions (PHP semantics) ────────────────────────────────────

    /// PHP truthiness: `false`, `0`, `0.0`, `""`, `"0"`, `null`, and the empty
    /// array are falsy; everything else is truthy. A closure/object handle is
    /// always truthy (only the empty *array* is falsy among heap objects).
    pub fn is_truthy(&self, v: &Value) -> bool {
        match v {
            Value::Undef => false,
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !(s.is_empty() || s.as_str() == "0"),
            Value::Obj(_) => !self.is_array(v) || self.array_len(v) != 0,
            _ => true,
        }
    }

    /// PHP string cast (`(string)` / echo / interpolation).
    /// [`PhpHost::to_str`] for a conversion the PROGRAM asked for, with the two
    /// diagnostics such a conversion raises.
    ///
    /// Both are properties of the conversion, not of the value, so they belong
    /// at the one point every PHP-visible string coercion passes through rather
    /// than at each of its callers: `echo`, `.`, `"$x"`, `(string)`, `strval`,
    /// `implode` and `%s` all raise them, while `var_dump`, `json_encode`,
    /// `in_array` and a loose comparison against a string raise neither, because
    /// none of those converts anything.
    ///
    /// An array has no string form, so the reference substitutes the literal
    /// text `Array` and warns. A NaN has a string form — `"NAN"` — and warns
    /// anyway, because the text does not read back as a number. The infinities
    /// are the control that shows this is about NaN specifically and not about
    /// non-finite doubles: `(string) INF` is `"INF"` and says nothing.
    pub fn to_str_diag(&mut self, v: &Value) -> String {
        match v {
            Value::Obj(_) if self.is_array(v) => self.warn("Array to string conversion"),
            Value::Float(f) if f.is_nan() => {
                self.warn("unexpected NAN value was coerced to string");
            }
            _ => {}
        }
        self.to_str(v)
    }

    /// A value as a PHP string.
    ///
    /// The `Str` arm CLONES the buffer rather than going through `to_string()`.
    /// They are not the same call: `Value::Str` holds an `Arc<String>`, which
    /// `ToString` has no specialisation for, so `s.to_string()` ran the whole
    /// `core::fmt` machinery — `Display::fmt` → `Write::write_str` →
    /// `push_str` — for every string that reaches a concatenation, an
    /// interpolation, an array key or a library argument. A sampling profile of
    /// `$s .= "x$i,"` spent most of its VM time there.
    pub fn to_str(&self, v: &Value) -> String {
        match v {
            Value::Undef => String::new(),
            Value::Bool(b) => if *b { "1" } else { "" }.to_string(),
            Value::Int(n) => n.to_string(),
            Value::Float(f) => float_to_php_string(*f),
            Value::Str(s) => String::clone(s),
            Value::Obj(_) => match self.resource_info(v) {
                Some((id, _)) => format!("Resource id #{id}"),
                None => "Array".to_string(),
            },
            _ => String::new(),
        }
    }

    /// Step an int by `delta`, widening to float on overflow the way PHP does.
    /// `$x = PHP_INT_MAX; $x++;` leaves a float, not a wrapped negative — the
    /// engine has no integer overflow, so the type changes instead.
    fn int_step_impl(n: i64, delta: i64) -> Value {
        match n.checked_add(delta) {
            Some(v) => Value::int(v),
            None => Value::float(n as f64 + delta as f64),
        }
    }

    /// `++`/`--` applied to one value, with the diagnostics PHP raises for the
    /// operand types the operators do not actually change.
    ///
    /// The operators are not arithmetic on `$x + 1`: `null--` and `true++` leave
    /// the value alone, `""++` produces the *string* `"1"`, and `++` on a
    /// non-numeric string is Perl-style alphanumeric succession (`"Az"` → `"Ba"`,
    /// `"zz"` → `"aaa"`) rather than a numeric coercion. Only a numeric string
    /// goes through the number path.
    pub fn incdec_value(&mut self, old: &Value, inc: bool) -> Value {
        let delta = if inc { 1 } else { -1 };
        match old {
            // null++ is 1; null-- is a no-op PHP has announced it will change.
            Value::Undef => {
                if inc {
                    Value::int(1)
                } else {
                    self.warn(
                        "Decrement on type null has no effect, this will change in the next \
                         major version of PHP",
                    );
                    Value::Undef
                }
            }
            // Neither operator has ever affected a bool.
            Value::Bool(_) => {
                let word = if inc { "Increment" } else { "Decrement" };
                self.warn(format_args!(
                    "{word} on type bool has no effect, this will change in the next major \
                     version of PHP"
                ));
                old.clone()
            }
            // Past the int range the operators WIDEN rather than wrap: PHP has
            // no integer overflow, so `PHP_INT_MAX + 1` is a float and stepping
            // an int off either end produces one.
            Value::Int(n) => Self::int_step_impl(*n, delta),
            Value::Float(f) => Value::float(f + delta as f64),
            Value::Str(s) if is_numeric_string(s) => match parse_php_number(s) {
                Value::Float(f) => Value::float(f + delta as f64),
                Value::Int(n) => Self::int_step_impl(n, delta),
                other => other,
            },
            Value::Str(s) if s.is_empty() => {
                if inc {
                    self.deprecated(
                        "Increment on non-numeric string is deprecated, use str_increment() \
                         instead",
                    );
                    Value::str("1")
                } else {
                    self.deprecated("Decrement on empty string is deprecated as non-numeric");
                    Value::int(-1)
                }
            }
            Value::Str(s) => {
                if inc {
                    self.deprecated(
                        "Increment on non-numeric string is deprecated, use str_increment() \
                         instead",
                    );
                    Value::str(increment_alnum_string(s))
                } else {
                    self.deprecated(
                        "Decrement on non-numeric string has no effect and is deprecated",
                    );
                    old.clone()
                }
            }
            other => other.clone(),
        }
    }

    /// Coerce to an `Int`/`Float` `Value` for arithmetic and comparison.
    pub fn to_number(&self, v: &Value) -> Value {
        match v {
            Value::Int(_) | Value::Float(_) => v.clone(),
            Value::Bool(b) => Value::int(*b as i64),
            Value::Undef => Value::int(0),
            Value::Str(s) => parse_php_number(s),
            // A resource casts to its number.
            Value::Obj(_) if self.is_stream(v) => {
                Value::int(self.resource_info(v).map_or(0, |(id, _)| id))
            }
            // Empty array → 0, non-empty array → 1; a closure/object casts to 1.
            Value::Obj(_) => Value::int(if self.is_array(v) && self.array_len(v) == 0 {
                0
            } else {
                1
            }),
            _ => Value::int(0),
        }
    }

    /// PHP `gettype` name.
    pub fn type_name(&self, v: &Value) -> &'static str {
        match v {
            Value::Undef => "NULL",
            Value::Bool(_) => "boolean",
            Value::Int(_) => "integer",
            Value::Float(_) => "double",
            Value::Str(_) => "string",
            Value::Obj(_) => {
                if self.is_array(v) {
                    "array"
                } else if let Some((_, closed)) = self.resource_info(v) {
                    if closed {
                        "resource (closed)"
                    } else {
                        "resource"
                    }
                } else {
                    "object"
                }
            }
            _ => "unknown type",
        }
    }
}

/// The standard predefined constants seeded onto every fresh host. Covers the
/// core/version/OS constants, the `M_*` math constants, and the integer flag
/// constants the standard library accepts (sort/count/str_pad/array_filter/preg/
/// json/filter/mbstring/file/pathinfo/entities/error-level families), so a
/// program that writes `SORT_STRING` or `FILTER_VALIDATE_EMAIL` gets the real
/// integer rather than the bare name.
/// Whether a variable name (sans `$`) is a PHP superglobal — auto-global across
/// every scope, resolved against the global frame.
pub fn is_superglobal(name: &str) -> bool {
    // First-byte gate before the eleven comparisons. This is asked on every
    // by-name variable access — it was 3% of a `foreach` profile — and the
    // answer is almost always no, which one byte settles for every name that
    // does not begin like a superglobal.
    if !matches!(name.as_bytes().first(), Some(b'_' | b'G' | b'a')) {
        return false;
    }
    matches!(
        name,
        "_SERVER"
            | "_GET"
            | "_POST"
            | "_REQUEST"
            | "_COOKIE"
            | "_FILES"
            | "_ENV"
            | "_SESSION"
            | "GLOBALS"
            | "argv"
            | "argc"
    )
}

fn predefined_constants() -> FxHashMap<String, Value> {
    let mut m = FxHashMap::default();
    let mut si = |k: &str, v: i64| {
        m.insert(k.to_string(), Value::int(v));
    };
    // core / platform
    si("PHP_INT_MAX", i64::MAX);
    si("PHP_INT_MIN", i64::MIN);
    si("PHP_INT_SIZE", 8);
    si("PHP_FLOAT_DIG", 15);
    si("PHP_MAJOR_VERSION", 8);
    si("PHP_MINOR_VERSION", 3);
    si("PHP_RELEASE_VERSION", 0);
    si("PHP_VERSION_ID", 80300);
    si("PHP_ROUND_HALF_UP", 1);
    si("PHP_ROUND_HALF_DOWN", 2);
    si("PHP_ROUND_HALF_EVEN", 3);
    si("PHP_ROUND_HALF_ODD", 4);
    // Error levels — the canonical values live in `crate::errlevel`, which the
    // `error_reporting` mask and the `-d error_reporting=…` parser share.
    si("E_ERROR", errlevel::E_ERROR);
    si("E_WARNING", errlevel::E_WARNING);
    si("E_PARSE", errlevel::E_PARSE);
    si("E_NOTICE", errlevel::E_NOTICE);
    si("E_CORE_ERROR", errlevel::E_CORE_ERROR);
    si("E_CORE_WARNING", errlevel::E_CORE_WARNING);
    si("E_COMPILE_ERROR", errlevel::E_COMPILE_ERROR);
    si("E_COMPILE_WARNING", errlevel::E_COMPILE_WARNING);
    si("E_STRICT", errlevel::E_STRICT);
    si("E_RECOVERABLE_ERROR", errlevel::E_RECOVERABLE_ERROR);
    si("E_DEPRECATED", errlevel::E_DEPRECATED);
    si("E_ALL", errlevel::E_ALL);
    si("E_USER_ERROR", errlevel::E_USER_ERROR);
    si("E_USER_WARNING", errlevel::E_USER_WARNING);
    si("E_USER_NOTICE", errlevel::E_USER_NOTICE);
    si("E_USER_DEPRECATED", errlevel::E_USER_DEPRECATED);
    // sort / count / str_pad / array_filter
    si("SORT_REGULAR", 0);
    si("SORT_NUMERIC", 1);
    si("SORT_STRING", 2);
    si("SORT_DESC", 3);
    si("SORT_ASC", 4);
    si("SORT_LOCALE_STRING", 5);
    si("SORT_NATURAL", 6);
    si("SORT_FLAG_CASE", 8);
    // output control (main/php_output.h)
    si("PHP_OUTPUT_HANDLER_START", 1);
    si("PHP_OUTPUT_HANDLER_WRITE", 0);
    si("PHP_OUTPUT_HANDLER_FLUSH", 4);
    si("PHP_OUTPUT_HANDLER_CLEAN", 2);
    si("PHP_OUTPUT_HANDLER_FINAL", 8);
    si("PHP_OUTPUT_HANDLER_CONT", 0);
    si("PHP_OUTPUT_HANDLER_END", 8);
    si("PHP_OUTPUT_HANDLER_CLEANABLE", 16);
    si("PHP_OUTPUT_HANDLER_FLUSHABLE", 32);
    si("PHP_OUTPUT_HANDLER_REMOVABLE", 64);
    si("PHP_OUTPUT_HANDLER_STDFLAGS", 112);
    si("PHP_OUTPUT_HANDLER_STARTED", 4096);
    si("PHP_OUTPUT_HANDLER_DISABLED", 8192);
    si("PHP_OUTPUT_HANDLER_PROCESSED", 16384);
    si("COUNT_NORMAL", 0);
    si("COUNT_RECURSIVE", 1);
    si("STR_PAD_RIGHT", 1);
    si("STR_PAD_LEFT", 0);
    si("STR_PAD_BOTH", 2);
    si("ARRAY_FILTER_USE_KEY", ARRAY_FILTER_USE_KEY);
    si("ARRAY_FILTER_USE_BOTH", ARRAY_FILTER_USE_BOTH);
    // extract() — the low values pick one mode, EXTR_REFS is an OR-able bit.
    si("EXTR_OVERWRITE", 0);
    si("EXTR_SKIP", 1);
    si("EXTR_PREFIX_SAME", 2);
    si("EXTR_PREFIX_ALL", 3);
    si("EXTR_PREFIX_INVALID", 4);
    si("EXTR_PREFIX_IF_EXISTS", 5);
    si("EXTR_IF_EXISTS", 6);
    si("EXTR_REFS", 256);
    // locale categories: the C library's own numbering, which differs by platform
    si("LC_CTYPE", i64::from(libc::LC_CTYPE));
    si("LC_NUMERIC", i64::from(libc::LC_NUMERIC));
    si("LC_TIME", i64::from(libc::LC_TIME));
    si("LC_COLLATE", i64::from(libc::LC_COLLATE));
    si("LC_MONETARY", i64::from(libc::LC_MONETARY));
    si("LC_MESSAGES", i64::from(libc::LC_MESSAGES));
    si("LC_ALL", i64::from(libc::LC_ALL));
    // preg
    si("PREG_PATTERN_ORDER", 1);
    si("PREG_SET_ORDER", 2);
    si("PREG_OFFSET_CAPTURE", 256);
    si("PREG_UNMATCHED_AS_NULL", 512);
    si("PREG_SPLIT_NO_EMPTY", 1);
    si("PREG_SPLIT_DELIM_CAPTURE", 2);
    si("PREG_SPLIT_OFFSET_CAPTURE", 4);
    si("PREG_GREP_INVERT", 1);
    // The `preg_last_error()` codes, whose names are what user code compares
    // against rather than the bare numbers.
    si("PREG_NO_ERROR", 0);
    si("PREG_INTERNAL_ERROR", 1);
    si("PREG_BACKTRACK_LIMIT_ERROR", 2);
    si("PREG_RECURSION_LIMIT_ERROR", 3);
    si("PREG_BAD_UTF8_ERROR", 4);
    si("PREG_BAD_UTF8_OFFSET_ERROR", 5);
    si("PREG_JIT_STACKLIMIT_ERROR", 6);
    // json
    si("JSON_HEX_TAG", 1);
    si("JSON_HEX_AMP", 2);
    si("JSON_HEX_APOS", 4);
    si("JSON_HEX_QUOT", 8);
    si("JSON_FORCE_OBJECT", 16);
    si("JSON_NUMERIC_CHECK", 32);
    si("JSON_UNESCAPED_SLASHES", 64);
    si("JSON_PRETTY_PRINT", 128);
    si("JSON_UNESCAPED_UNICODE", 256);
    si("JSON_PARTIAL_OUTPUT_ON_ERROR", 512);
    si("JSON_PRESERVE_ZERO_FRACTION", 1024);
    si("JSON_UNESCAPED_LINE_TERMINATORS", 2048);
    si("JSON_THROW_ON_ERROR", 4194304);
    si("JSON_OBJECT_AS_ARRAY", 1);
    si("JSON_BIGINT_AS_STRING", 2);
    si("JSON_ERROR_NONE", 0);
    si("JSON_ERROR_DEPTH", 1);
    si("JSON_ERROR_STATE_MISMATCH", 2);
    si("JSON_ERROR_CTRL_CHAR", 3);
    si("JSON_ERROR_SYNTAX", 4);
    si("JSON_ERROR_UTF8", 5);
    si("JSON_ERROR_RECURSION", 6);
    si("JSON_ERROR_INF_OR_NAN", 7);
    si("JSON_ERROR_UNSUPPORTED_TYPE", 8);
    si("JSON_ERROR_INVALID_PROPERTY_NAME", 9);
    si("JSON_ERROR_UTF16", 10);
    si("JSON_ERROR_NON_BACKED_ENUM", 11);
    // url — `parse_url()`'s `$component` selectors (ext/standard/url.h).
    si("PHP_URL_SCHEME", 0);
    si("PHP_URL_HOST", 1);
    si("PHP_URL_PORT", 2);
    si("PHP_URL_USER", 3);
    si("PHP_URL_PASS", 4);
    si("PHP_URL_PATH", 5);
    si("PHP_URL_QUERY", 6);
    si("PHP_URL_FRAGMENT", 7);
    // filter
    si("INPUT_GET", 1);
    si("INPUT_POST", 0);
    si("FILTER_DEFAULT", 516);
    si("FILTER_UNSAFE_RAW", 516);
    si("FILTER_VALIDATE_INT", 257);
    si("FILTER_VALIDATE_BOOLEAN", 258);
    si("FILTER_VALIDATE_BOOL", 258);
    si("FILTER_VALIDATE_FLOAT", 259);
    si("FILTER_VALIDATE_REGEXP", 272);
    si("FILTER_VALIDATE_DOMAIN", 277);
    si("FILTER_VALIDATE_URL", 273);
    si("FILTER_VALIDATE_EMAIL", 274);
    si("FILTER_VALIDATE_IP", 275);
    si("FILTER_VALIDATE_MAC", 276);
    si("FILTER_SANITIZE_STRING", 513);
    si("FILTER_SANITIZE_STRIPPED", 513);
    si("FILTER_SANITIZE_ENCODED", 514);
    si("FILTER_SANITIZE_SPECIAL_CHARS", 515);
    si("FILTER_SANITIZE_FULL_SPECIAL_CHARS", 522);
    si("FILTER_SANITIZE_EMAIL", 517);
    si("FILTER_SANITIZE_URL", 518);
    si("FILTER_SANITIZE_NUMBER_INT", 519);
    si("FILTER_SANITIZE_NUMBER_FLOAT", 520);
    si("FILTER_SANITIZE_ADD_SLASHES", 523);
    si("FILTER_FLAG_ALLOW_OCTAL", 1);
    si("FILTER_FLAG_ALLOW_HEX", 2);
    si("FILTER_FLAG_STRIP_LOW", 4);
    si("FILTER_FLAG_STRIP_HIGH", 8);
    si("FILTER_FLAG_ALLOW_FRACTION", 4096);
    si("FILTER_FLAG_ALLOW_THOUSAND", 8192);
    si("FILTER_FLAG_ALLOW_SCIENTIFIC", 16384);
    si("FILTER_FLAG_IPV4", 1048576);
    si("FILTER_FLAG_IPV6", 2097152);
    si("FILTER_FLAG_HOSTNAME", 1048576);
    si("FILTER_NULL_ON_FAILURE", 134217728);
    si("FILTER_REQUIRE_SCALAR", 33554432);
    si("FILTER_REQUIRE_ARRAY", 16777216);
    si("FILTER_FORCE_ARRAY", 67108864);
    // mbstring
    si("MB_CASE_UPPER", 0);
    si("MB_CASE_LOWER", 1);
    si("MB_CASE_TITLE", 2);
    // file / dir
    si("FILE_USE_INCLUDE_PATH", 1);
    si("FILE_APPEND", 8);
    si("FILE_IGNORE_NEW_LINES", 2);
    si("FILE_SKIP_EMPTY_LINES", 4);
    si("FILE_NO_DEFAULT_CONTEXT", 16);
    si("LOCK_SH", 1);
    si("LOCK_EX", 2);
    si("LOCK_UN", 3);
    si("LOCK_NB", 4);
    // fseek whence
    si("SEEK_SET", 0);
    si("SEEK_CUR", 1);
    si("SEEK_END", 2);
    si("SCANDIR_SORT_ASCENDING", 0);
    si("SCANDIR_SORT_DESCENDING", 1);
    si("SCANDIR_SORT_NONE", 2);
    // glob / fnmatch. The values live next to the matcher that reads them, in
    // `stdlib::fileio`, so a flag cannot be seeded with one bit and tested with
    // another — which is what had happened before these were seeded at all.
    si("GLOB_ERR", crate::stdlib::fileio::GLOB_ERR);
    si("GLOB_MARK", crate::stdlib::fileio::GLOB_MARK);
    si("GLOB_NOCHECK", crate::stdlib::fileio::GLOB_NOCHECK);
    si("GLOB_NOSORT", crate::stdlib::fileio::GLOB_NOSORT);
    si("GLOB_BRACE", crate::stdlib::fileio::GLOB_BRACE);
    si("GLOB_NOESCAPE", crate::stdlib::fileio::GLOB_NOESCAPE);
    si("GLOB_ONLYDIR", crate::stdlib::fileio::GLOB_ONLYDIR);
    si(
        "GLOB_AVAILABLE_FLAGS",
        crate::stdlib::fileio::GLOB_AVAILABLE_FLAGS,
    );
    si("FNM_NOESCAPE", crate::stdlib::fileio::FNM_NOESCAPE);
    si("FNM_PATHNAME", crate::stdlib::fileio::FNM_PATHNAME);
    si("FNM_PERIOD", crate::stdlib::fileio::FNM_PERIOD);
    si("FNM_CASEFOLD", crate::stdlib::fileio::FNM_CASEFOLD);
    si("PATHINFO_DIRNAME", 1);
    si("PATHINFO_BASENAME", 2);
    si("PATHINFO_EXTENSION", 4);
    si("PATHINFO_FILENAME", 8);
    si("PATHINFO_ALL", 15);
    // array_change_key_case
    si("CASE_LOWER", 0);
    si("CASE_UPPER", 1);
    // html entities
    si("HTML_SPECIALCHARS", 0);
    si("HTML_ENTITIES", 1);
    si("ENT_NOQUOTES", 0);
    si("ENT_COMPAT", 2);
    si("ENT_QUOTES", 3);
    si("ENT_HTML401", 0);
    si("ENT_HTML5", 48);
    si("ENT_XML1", 16);
    si("ENT_XHTML", 32);
    si("ENT_SUBSTITUTE", 8);
    si("ENT_IGNORE", 4);
    si("SUNFUNCS_RET_TIMESTAMP", 0);
    si("SUNFUNCS_RET_STRING", 1);
    si("SUNFUNCS_RET_DOUBLE", 2);
    // string constants
    let mut ss = |k: &str, v: &str| {
        m.insert(k.to_string(), Value::str(v.to_string()));
    };
    ss("PHP_EOL", "\n");
    // ext/date's format constants, the same strings as `DateTimeInterface::*`.
    for (k, v) in [
        ("DATE_ATOM", r"Y-m-d\TH:i:sP"),
        ("DATE_COOKIE", "l, d-M-Y H:i:s T"),
        ("DATE_ISO8601", r"Y-m-d\TH:i:sO"),
        ("DATE_ISO8601_EXPANDED", r"X-m-d\TH:i:sP"),
        ("DATE_RFC822", "D, d M y H:i:s O"),
        ("DATE_RFC850", "l, d-M-y H:i:s T"),
        ("DATE_RFC1036", "D, d M y H:i:s O"),
        ("DATE_RFC1123", "D, d M Y H:i:s O"),
        ("DATE_RFC7231", r"D, d M Y H:i:s \G\M\T"),
        ("DATE_RFC2822", "D, d M Y H:i:s O"),
        ("DATE_RFC3339", r"Y-m-d\TH:i:sP"),
        ("DATE_RFC3339_EXTENDED", r"Y-m-d\TH:i:s.vP"),
        ("DATE_RSS", "D, d M Y H:i:s O"),
        ("DATE_W3C", r"Y-m-d\TH:i:sP"),
    ] {
        ss(k, v);
    }
    ss("PHP_VERSION", "8.3.0");
    ss(
        "PHP_OS",
        if cfg!(target_os = "macos") {
            "Darwin"
        } else if cfg!(target_os = "windows") {
            "WINNT"
        } else {
            "Linux"
        },
    );
    ss(
        "PHP_OS_FAMILY",
        if cfg!(target_os = "windows") {
            "Windows"
        } else if cfg!(target_os = "macos") {
            "Darwin"
        } else {
            "Linux"
        },
    );
    ss(
        "DIRECTORY_SEPARATOR",
        if cfg!(windows) { "\\" } else { "/" },
    );
    ss("PATH_SEPARATOR", if cfg!(windows) { ";" } else { ":" });
    // math constants
    let mut sf = |k: &str, v: f64| {
        m.insert(k.to_string(), Value::float(v));
    };
    sf("M_PI", std::f64::consts::PI);
    sf("M_E", std::f64::consts::E);
    sf("M_SQRT2", std::f64::consts::SQRT_2);
    sf("M_SQRT1_2", std::f64::consts::FRAC_1_SQRT_2);
    sf("M_SQRT3", 1.7320508075688772);
    sf("M_2_SQRTPI", std::f64::consts::FRAC_2_SQRT_PI);
    sf("M_PI_2", std::f64::consts::FRAC_PI_2);
    sf("M_PI_4", std::f64::consts::FRAC_PI_4);
    sf("M_1_PI", std::f64::consts::FRAC_1_PI);
    sf("M_2_PI", std::f64::consts::FRAC_2_PI);
    sf("M_LN2", std::f64::consts::LN_2);
    sf("M_LN10", std::f64::consts::LN_10);
    sf("M_LOG2E", std::f64::consts::LOG2_E);
    sf("M_LOG10E", std::f64::consts::LOG10_E);
    sf("M_EULER", 0.5772156649015329);
    sf("M_SQRTPI", 1.7724538509055159);
    sf("PHP_FLOAT_EPSILON", f64::EPSILON);
    sf("PHP_FLOAT_MAX", f64::MAX);
    sf("PHP_FLOAT_MIN", f64::MIN_POSITIVE);
    sf("INF", f64::INFINITY);
    sf("NAN", f64::NAN);
    m
}

// ── thread-local host access ──────────────────────────────────────────────

thread_local! {
    static HOST: RefCell<PhpHost> = RefCell::new(PhpHost::new());

    /// The source line a diagnostic raised right now belongs to. The builtin
    /// that is about to warn records it from the line table of the op it is
    /// executing, so the host never has to walk the VM.
    ///
    /// It lives OUTSIDE `HOST` because it is written far more often than it is
    /// read: every variable read, every array subscript, and every arithmetic
    /// operator stamps it so that a diagnostic it MIGHT raise names the right
    /// line, and hardly any of them raises one. Through `HOST` each of those
    /// stamps borrowed the entire host through a `RefCell`; this is a store.
    static WARN_LINE: Cell<u32> = const { Cell::new(0) };
}

/// Record the line a diagnostic raised next belongs to.
pub fn set_warn_line(line: u32) {
    WARN_LINE.with(|c| c.set(line));
}

/// The line [`set_warn_line`] last recorded.
pub fn warn_line() -> u32 {
    WARN_LINE.with(Cell::get)
}

/// Run `f` with mutable access to the current thread's `PhpHost`.
pub fn with_host<R>(f: impl FnOnce(&mut PhpHost) -> R) -> R {
    HOST.with(|h| f(&mut h.borrow_mut()))
}

/// Reset the host to a fresh state (new heap, new global scope).
pub fn reset_host() {
    CUR_GEN.with(|c| c.set(None));
    crate::stdlib::system::reset_lc_ctype();
    set_warn_line(0);
    HOST.with(|h| *h.borrow_mut() = PhpHost::new());
}

// ── generators (host-side stackful coroutines) ──────────────────────────────
//
// A generator function's body runs on its own native stack inside a
// `corosensei::Coroutine`. `run_chunk_on` (the fusevm VM run loop) executes *on
// that stack*, so a `yield` deep inside the body suspends the whole VM back to
// the resumer with one stack switch — no fusevm change. Same design the sibling
// node-js frontend uses. Single-threaded: the coroutine shares the thread-local
// `HOST`, and no `with_host` borrow is ever held across a `resume`/`suspend`.
//
// The coroutine yields `()` (the current key/value are stashed in the `GenCell`),
// is resumed with a `Value` (the `->send($x)` argument), and returns
// `Result<Value, String>` (the generator body's `return` value, or an error).
// corosensei's generics are `Coroutine<Input, Yield, Return>`: the body is resumed
// with `Input` (the `->send($x)` value), yields `Yield` (here `()` — the current
// key/value ride in the `GenCell`), and finally returns `Return`.
type GenCoro = corosensei::Coroutine<Value, (), Result<Value, String>>;
type GenYielder = corosensei::Yielder<Value, ()>;

thread_local! {
    /// Id of the generator whose body is currently executing (the one a `yield`
    /// suspends), or `None` at the root / inside a non-generator call.
    static CUR_GEN: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

/// A forced completion injected at a suspended `yield`: `->throw()`, or the
/// generator being destroyed while suspended, which unwinds the body as a
/// `return` would — `finally` blocks run, `catch` blocks do not.
enum GenInject {
    Throw(Value),
    Destroy,
}

/// The error a `yield` reports when its generator is being destroyed. The
/// `return` signal is already set; the dispatcher only has to halt the chunk.
pub const GEN_DESTROY: &str = "__generator_destroy__";

/// The volatile execution state swapped in/out at every generator resume/suspend
/// boundary, so a suspended generator's half-finished call frame and in-flight
/// signal/throw never leak into the resumer (and vice versa). The global scope
/// (`scopes[0]`) is *not* part of this — it stays shared, so superglobals and
/// `global $x` work identically inside a generator.
#[derive(Default)]
struct GenContext {
    /// The generator's own call frames (everything above the global scope).
    frames: Vec<Scope>,
    signal: Option<Signal>,
    pending_throw: Option<Value>,
    error: Option<String>,
}

/// One live generator.
struct GenCell {
    /// The suspended coroutine. `None` only while it is actively running (taken
    /// out across `resume` so the body can re-borrow the host freely).
    coro: Option<GenCoro>,
    /// Raw pointer to the coroutine body's `Yielder`, published on entry. Valid
    /// for the whole body lifetime; only read from inside that body (its stack is
    /// live), on the same thread.
    yielder: *const GenYielder,
    ctx: GenContext,
    /// The most recently yielded key and value (read by `current()`/`key()`).
    cur_key: Value,
    cur_val: Value,
    /// The next auto-increment integer key (PHP numbers un-keyed yields 0,1,2…).
    auto_key: i64,
    /// The body's `return` value, once it has finished (`getReturn`).
    ret: Value,
    started: bool,
    /// `ZEND_GENERATOR_AT_FIRST_YIELD`: primed, and not resumed since — the
    /// only state `rewind()` accepts.
    at_first_yield: bool,
    done: bool,
    inject: Option<GenInject>,
    /// Being destroyed while suspended: a `yield` reached now (one in a
    /// `finally`) is an error rather than a suspension.
    destroying: bool,
}

impl PhpHost {
    pub fn is_generator_val(&self, v: &Value) -> bool {
        matches!(self.as_array(v), Some(PhpObj::Generator { .. }))
    }
    fn gen_id(&self, v: &Value) -> Option<u32> {
        match self.as_array(v) {
            Some(PhpObj::Generator { id }) => Some(*id),
            _ => None,
        }
    }
    /// Swap the volatile execution context in one shot, returning the previous one:
    /// installs a generator's frames/signal/throw on resume, pulls them back on
    /// suspend. The global scope (index 0) is left untouched, so superglobals and
    /// `global $x` stay shared.
    fn install_gen_ctx(&mut self, mut c: GenContext) -> GenContext {
        // Detach the caller's frames (everything above the global scope) and
        // install the generator's own frames in their place.
        let caller_frames = self.scopes.split_off(1);
        self.scopes.append(&mut c.frames); // drains c.frames into the active stack
        std::mem::swap(&mut self.signal, &mut c.signal);
        std::mem::swap(&mut self.pending_throw, &mut c.pending_throw);
        std::mem::swap(&mut self.error, &mut c.error);
        // `c` now carries the caller's saved signal/throw/error; hand it the
        // caller's frames so the reverse call restores them.
        c.frames = caller_frames;
        c
    }
}

/// Build a suspended generator over `body`, run in the already-bound call frame
/// `frame` (its parameters/captures set by the caller). Nothing runs until the
/// first resume.
fn make_generator(body: Chunk, frame: Scope) -> Value {
    let id = with_host(|h| {
        let id = h.generators.len() as u32;
        h.generators.push(GenCell {
            coro: None,
            yielder: std::ptr::null(),
            ctx: GenContext {
                frames: vec![frame],
                ..GenContext::default()
            },
            cur_key: Value::Undef,
            cur_val: Value::Undef,
            auto_key: 0,
            ret: Value::Undef,
            started: false,
            at_first_yield: false,
            done: false,
            inject: None,
            destroying: false,
        });
        id
    });
    let coro = GenCoro::new(move |yielder: &GenYielder, _first: Value| {
        // Same thread → publish the yielder so `yield` (deep in the body's VM) can
        // reach it. Valid for the whole body.
        with_host(|h| h.generators[id as usize].yielder = yielder as *const _);
        let r = run_chunk_on(body);
        // A `return v;` in the body leaves a Return signal; capture it as the
        // generator's completion value (`getReturn`).
        let ret = with_host(|h| match h.signal.take() {
            Some(Signal::Return(v)) => v,
            _ => Value::Undef,
        });
        r.map(|_| ret)
    });
    with_host(|h| h.generators[id as usize].coro = Some(coro));
    with_host(|h| {
        h.objs.push(PhpObj::Generator { id });
        Value::Obj((h.objs.len() - 1) as u32)
    })
}

/// How a `yield` computes its key.
enum YieldKey {
    /// `yield $v` — take (and advance) the next auto-increment integer key.
    Auto,
    /// `yield $k => $v` — an explicit key (an int at/above the counter advances it,
    /// matching array-append semantics).
    Explicit(Value),
    /// `yield from` passthrough — re-emit the delegate's key without touching the
    /// outer generator's auto-key counter.
    Passthrough(Value),
}

/// Suspend the running generator, publishing `(key, val)` to the resumer. Returns
/// the value the next `->send($x)` supplies (`Undef` for `->next()`), or unwinds a
/// `->throw()` injected at this point.
fn gen_yield(key: YieldKey, val: Value) -> Result<Value, String> {
    let id = match CUR_GEN.with(|c| c.get()) {
        Some(id) => id,
        None => return Err("cannot yield outside a generator".to_string()),
    };
    if with_host(|h| h.generators[id as usize].destroying) {
        return Err(crate::builtins::throws_bare(
            "Error",
            "Cannot yield from finally in a force-closed generator",
        ));
    }
    let yp = with_host(|h| {
        let g = &mut h.generators[id as usize];
        let k = match key {
            YieldKey::Auto => {
                let k = g.auto_key;
                g.auto_key += 1;
                Value::int(k)
            }
            YieldKey::Explicit(Value::Int(i)) => {
                if i >= g.auto_key {
                    g.auto_key = i + 1;
                }
                Value::int(i)
            }
            YieldKey::Explicit(other) => other,
            YieldKey::Passthrough(k) => k,
        };
        g.cur_key = k;
        g.cur_val = val;
        g.yielder
    });
    // SAFETY: same-thread coroutine; the yielder lives for the whole body and we
    // only reach here from inside that live body.
    let yielder = unsafe { &*yp };
    let sent = yielder.suspend(());
    match with_host(|h| h.generators[id as usize].inject.take()) {
        // Re-raise the injected exception at the suspension point so an enclosing
        // try/catch in the body handles it (or it unwinds the body).
        Some(GenInject::Throw(e)) => {
            set_pending_throw(e);
            return Err("__generator_throw__".to_string());
        }
        // Destroyed while parked here: leave the body as a `return;` would.
        Some(GenInject::Destroy) => {
            set_return(Value::Undef);
            return Err(GEN_DESTROY.to_string());
        }
        None => {}
    }
    Ok(sent)
}

/// `yield $v` — suspend with an auto-incremented integer key.
pub fn yield_value(val: Value) -> Result<Value, String> {
    gen_yield(YieldKey::Auto, val)
}

/// `yield $k => $v` — suspend with an explicit key.
pub fn yield_kv(key: Value, val: Value) -> Result<Value, String> {
    gen_yield(YieldKey::Explicit(key), val)
}

/// `yield from $src` — the public entry point (see `gen_yield_from`).
pub fn yield_from(src: Value) -> Result<Value, String> {
    gen_yield_from(src)
}

/// `yield from $src` — delegate to an array, a Generator, or any Traversable,
/// re-yielding each key/value from the enclosing generator. Evaluates to the
/// delegate's `return` value (null for an array or a returnless generator).
fn gen_yield_from(src: Value) -> Result<Value, String> {
    // A sub-generator is driven lazily (preserving side-effect order and passing
    // sent values through). Anything else is normalized to an array and replayed.
    if with_host(|h| h.is_generator_val(&src)) {
        gen_ensure_init(&src)?;
        loop {
            if !gen_valid(&src)? {
                break;
            }
            let k = with_host(|h| {
                h.generators[h.gen_id(&src).unwrap() as usize]
                    .cur_key
                    .clone()
            });
            let v = with_host(|h| {
                h.generators[h.gen_id(&src).unwrap() as usize]
                    .cur_val
                    .clone()
            });
            let sent = gen_yield(YieldKey::Passthrough(k), v)?;
            gen_send_raw(&src, sent)?;
        }
        return Ok(with_host(|h| {
            h.generators[h.gen_id(&src).unwrap() as usize].ret.clone()
        }));
    }
    let arr = foreach_prep(src)?;
    let pairs: Vec<(Value, Value)> = with_host(|h| match h.as_array(&arr) {
        Some(PhpObj::Array { entries, .. }) => entries
            .iter()
            .map(|(k, v)| (k.to_value(), v.clone()))
            .collect(),
        _ => Vec::new(),
    });
    for (k, v) in pairs {
        gen_yield(YieldKey::Passthrough(k), v)?;
    }
    Ok(Value::Undef)
}

/// Resume a generator until its next `yield` or its body returns. The coroutine is
/// taken out (so the body re-enters `with_host` freely) and the volatile context
/// is swapped so the caller's frames/signal survive the switch.
fn gen_resume(id: u32, send: Value) -> Result<(), String> {
    if with_host(|h| h.generators[id as usize].done) {
        return Ok(());
    }
    let mut coro = match with_host(|h| h.generators[id as usize].coro.take()) {
        Some(c) => c,
        None => return Err("cannot resume an already-running generator".to_string()),
    };
    with_host(|h| {
        let g = &mut h.generators[id as usize];
        if g.started {
            g.at_first_yield = false;
        }
        g.started = true;
    });
    let gen_ctx = with_host(|h| std::mem::take(&mut h.generators[id as usize].ctx));
    let caller_ctx = with_host(|h| h.install_gen_ctx(gen_ctx));
    let prev = CUR_GEN.with(|c| c.replace(Some(id)));

    let out = coro.resume(send); // no host borrow held; body drives its own VM

    CUR_GEN.with(|c| c.set(prev));
    // An uncaught exception in the body leaves a pending throw in the generator's
    // (currently installed) context. Take it out before the context is swapped
    // away so it can be re-raised in the resumer's context below.
    let body_throw = with_host(|h| h.pending_throw.take());
    let gen_ctx = with_host(|h| h.install_gen_ctx(caller_ctx));
    with_host(|h| {
        h.generators[id as usize].ctx = gen_ctx;
        h.generators[id as usize].coro = Some(coro);
    });

    let result = match out {
        corosensei::CoroutineResult::Yield(()) => Ok(()),
        corosensei::CoroutineResult::Return(r) => {
            with_host(|h| {
                let g = &mut h.generators[id as usize];
                g.done = true;
                g.cur_key = Value::Undef;
                g.cur_val = Value::Undef;
            });
            match r {
                Ok(v) => {
                    with_host(|h| h.generators[id as usize].ret = v);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
    };
    // Re-raise an uncaught body exception in the resumer's context.
    if let Some(e) = body_throw {
        set_pending_throw(e);
        return Err("__generator_throw__".to_string());
    }
    result
}

/// `zend_generator_ensure_initialized`: prime an unstarted generator to its
/// first `yield` (implicit on the first `current`/`valid`/`key`/`next`). A
/// no-op once started.
pub fn gen_ensure_init(gen: &Value) -> Result<(), String> {
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    if !with_host(|h| h.generators[id as usize].started) {
        gen_resume(id, Value::Undef)?;
        with_host(|h| h.generators[id as usize].at_first_yield = true);
    }
    Ok(())
}

/// `->rewind()` (`zend_generator_rewind`): prime the generator, and refuse one
/// already resumed past its first `yield` with the reference's `Exception`.
pub fn gen_rewind(gen: &Value) -> Result<(), String> {
    gen_ensure_init(gen)?;
    if with_host(|h| h.pending_throw.is_some()) {
        return Ok(());
    }
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    if !with_host(|h| h.generators[id as usize].at_first_yield) {
        let exc = new_object(
            "Exception",
            vec![Value::str("Cannot rewind a generator that was already run")],
        )?;
        set_pending_throw(exc);
    }
    Ok(())
}

/// A `foreach` (or `iterator_to_array`) over a generator: `zend_generator_get_iterator`
/// refuses a finished one, then the walk rewinds it.
pub fn gen_foreach_rewind(gen: &Value) -> Result<(), String> {
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    if with_host(|h| h.generators[id as usize].done) {
        let exc = new_object(
            "Exception",
            vec![Value::str("Cannot traverse an already closed generator")],
        )?;
        set_pending_throw(exc);
        return Ok(());
    }
    gen_rewind(gen)
}

/// `->valid()` — whether the generator has not yet finished (priming it first).
pub fn gen_valid(gen: &Value) -> Result<bool, String> {
    gen_ensure_init(gen)?;
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    Ok(with_host(|h| !h.generators[id as usize].done))
}

/// `->current()` — the current yielded value (priming first). Null once finished.
pub fn gen_current(gen: &Value) -> Result<Value, String> {
    gen_ensure_init(gen)?;
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    Ok(with_host(|h| h.generators[id as usize].cur_val.clone()))
}

/// `->key()` — the current yielded key (priming first). Null once finished.
pub fn gen_key(gen: &Value) -> Result<Value, String> {
    gen_ensure_init(gen)?;
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    Ok(with_host(|h| h.generators[id as usize].cur_key.clone()))
}

/// `->next()` — advance to the next yield (priming first, then resuming with null).
pub fn gen_next(gen: &Value) -> Result<Value, String> {
    gen_ensure_init(gen)?;
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    if !with_host(|h| h.generators[id as usize].done) {
        gen_resume(id, Value::Undef)?;
    }
    Ok(Value::Undef)
}

/// Resume `gen` with `sent` without the unstarted auto-prime — the internal step
/// used to pass a sent value down through `yield from`.
fn gen_send_raw(gen: &Value, sent: Value) -> Result<(), String> {
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    if !with_host(|h| h.generators[id as usize].done) {
        gen_resume(id, sent)?;
    }
    Ok(())
}

/// `->send($v)` — resume the generator, making the paused `yield` evaluate to `$v`,
/// and return the next yielded value (null once finished). On an unstarted
/// generator PHP first primes to the first yield, then sends.
pub fn gen_send(gen: &Value, sent: Value) -> Result<Value, String> {
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    if !with_host(|h| h.generators[id as usize].started) {
        gen_resume(id, Value::Undef)?;
    }
    if !with_host(|h| h.generators[id as usize].done) {
        gen_resume(id, sent)?;
    }
    Ok(with_host(|h| h.generators[id as usize].cur_val.clone()))
}

/// `->throw($e)` — raise `$e` at the current suspension point (running an enclosing
/// try/catch/finally in the body), then return the next yielded value.
pub fn gen_throw(gen: &Value, e: Value) -> Result<Value, String> {
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    gen_ensure_init(gen)?;
    if with_host(|h| h.generators[id as usize].done) {
        // Throw into a finished generator: it propagates straight to the caller.
        set_pending_throw(e);
        return Ok(Value::Undef);
    }
    with_host(|h| h.generators[id as usize].inject = Some(GenInject::Throw(e)));
    gen_resume(id, Value::Undef)?;
    Ok(with_host(|h| h.generators[id as usize].cur_val.clone()))
}

/// Request shutdown: destroy every generator still suspended, in the order they
/// were created, so the `finally` blocks around each one's parked `yield` run —
/// after an `exit` and after an uncaught exception's fatal as well, which is
/// when the reference frees them.
///
/// The reference first frees the global variables that alone hold an object,
/// newest first, and only then the rest in creation order. Globals held in
/// fusevm frame slots are gone by now, so that first pass cannot be
/// reproduced: two generators parked in two globals finish in creation order
/// here. An `exit` status already set is parked across the destruction, or
/// each body would see the run unwinding and skip its `finally`; one that a
/// `finally` sets itself wins.
pub fn shutdown_generators() {
    let exit = with_host(|h| h.pending_exit.take());
    let count = with_host(|h| h.generators.len());
    for id in 0..count as u32 {
        if gen_destroy(id).is_err() || unwinding() {
            break;
        }
    }
    with_host(|h| {
        if h.pending_exit.is_none() {
            h.pending_exit = exit;
        }
    });
    crate::stdlib::output::end_all();
}

/// [`ops::GEN_MARK`]: the id the next generator created will get.
pub fn gen_mark() -> i64 {
    with_host(|h| h.generators.len() as i64)
}

/// Destroy a generator: one suspended at a `yield` is resumed with a forced
/// `return`, so the `finally` blocks enclosing that `yield` run (an exception
/// one throws reaches the destroyer). An unstarted or finished one has nothing
/// to run. Either way it is finished afterwards.
fn gen_destroy(id: u32) -> Result<(), String> {
    let live = with_host(|h| {
        let g = &h.generators[id as usize];
        g.started && !g.done && g.coro.is_some()
    });
    let r = if live {
        with_host(|h| {
            let g = &mut h.generators[id as usize];
            g.destroying = true;
            g.inject = Some(GenInject::Destroy);
        });
        gen_resume(id, Value::Undef)
    } else {
        Ok(())
    };
    with_host(|h| {
        let g = &mut h.generators[id as usize];
        g.done = true;
        g.cur_key = Value::Undef;
        g.cur_val = Value::Undef;
    });
    r
}

/// [`ops::GEN_RELEASE`]: destroy `v` if it is a suspended generator that
/// nothing reachable holds any more.
///
/// Only a generator created at or after `mark` qualifies: one the `foreach`
/// subject expression itself produced. A local of a frame that is still
/// running can live in a fusevm frame slot this scan cannot see, but no such
/// local can have received a generator its callee created and returned —
/// that is what makes the scan below sufficient for a fresh one, where it
/// would not be for `foreach ($g as …)`.
///
/// phplang keeps no reference counts, so "nothing holds it" is answered by
/// looking: every frame's variables (the running ones and those parked in
/// other generators), reference cells, static and constant values, and every
/// array element, property and closure capture on the heap. A handle found
/// anywhere keeps the generator alive — a stale one in a dead array included,
/// which errs toward NOT running a `finally` early.
pub fn gen_release(v: &Value, mark: i64) -> Result<(), String> {
    let Some((id, handle)) = with_host(|h| match v {
        Value::Obj(o) => h.gen_id(v).map(|id| (id, *o)),
        _ => None,
    }) else {
        return Ok(());
    };
    let pending = with_host(|h| {
        let g = &h.generators[id as usize];
        i64::from(id) >= mark && g.started && !g.done
    });
    if !pending || with_host(|h| h.handle_held(handle)) {
        return Ok(());
    }
    gen_destroy(id)
}

impl PhpHost {
    /// Whether any variable, reference cell, static, constant, array element,
    /// property or closure binding holds the object handle `handle`.
    fn handle_held(&self, handle: u32) -> bool {
        let is = |v: &Value| matches!(v, Value::Obj(o) if *o == handle);
        let frame_holds = |s: &Scope| {
            s.vars
                .slots
                .iter()
                .any(|slot| matches!(slot, Slot::Val(v) if is(v)))
        };
        self.scopes.iter().any(frame_holds)
            || self
                .generators
                .iter()
                .any(|g| g.ctx.frames.iter().any(frame_holds))
            || self.ref_cells.iter().any(is)
            || self.static_props.values().any(is)
            || self.constants.values().any(is)
            || self.objs.iter().any(|o| match o {
                PhpObj::Array { entries, .. } => entries.values().any(is),
                PhpObj::Object { props, .. } => props.values().any(is),
                PhpObj::Closure {
                    captured,
                    bound_this,
                    ..
                } => captured.iter().any(|(_, v)| is(v)) || bound_this.as_ref().is_some_and(is),
                _ => false,
            })
    }
}

/// `->getReturn()` — the value the body `return`ed.
///
/// Asking BEFORE the body has returned is an error rather than a null: the
/// reference cannot distinguish "returned nothing" from "has not returned yet"
/// by value, so it refuses the question instead of answering it wrongly. A
/// generator that ran to completion without a `return` answers null.
pub fn gen_get_return(gen: &Value) -> Result<Value, String> {
    let id = with_host(|h| h.gen_id(gen)).ok_or("not a generator")?;
    let (done, ret) = with_host(|h| {
        let g = &h.generators[id as usize];
        (g.done, g.ret.clone())
    });
    if !done {
        // Raised through the internal-throw path so the trace carries the
        // `Generator->getReturn()` frame the reference prints for it.
        return throw_from_internal(
            "Generator->getReturn",
            &[],
            "Exception",
            "Cannot get return value of a generator that hasn't returned",
        );
    }
    Ok(ret)
}

/// Dispatch a `$gen->method(...)` call on a Generator object.
pub fn call_generator_method(gen: &Value, method: &str, args: Vec<Value>) -> Result<Value, String> {
    let mut args = args;
    match method.to_ascii_lowercase().as_str() {
        "current" => gen_current(gen),
        "key" => gen_key(gen),
        "next" => gen_next(gen),
        "valid" => gen_valid(gen).map(Value::bool),
        "rewind" => gen_rewind(gen).map(|_| Value::Undef),
        "send" => gen_send(gen, args.into_iter().next().unwrap_or(Value::Undef)),
        "throw" => gen_throw(gen, args.into_iter().next().unwrap_or(Value::Undef)),
        "getreturn" => gen_get_return(gen),
        other => {
            let _ = args.pop();
            Err(crate::builtins::throws_bare(
                "Error",
                format!("Call to undefined method Generator::{other}()"),
            ))
        }
    }
}

// ── execution ─────────────────────────────────────────────────────────────

thread_local! {
    static DEBUG_MODE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };

    /// Recycled VMs for [`run_chunk_on`].
    ///
    /// Every PHP function call, method call, closure invocation, constant
    /// initializer and `eval` runs its own chunk on its own VM. Building one
    /// meant `VM::new` plus `builtins::install`, which is 120 `register_builtin`
    /// calls each growing the table — per CALL. `VM::reset` keeps the builtin
    /// table (and the allocations behind the stack, frames and globals), so a
    /// recycled VM starts with all of that already in place.
    ///
    /// A stack, not a single slot, because calls nest: a recursive PHP function
    /// holds its VM for the whole call, and a suspended generator holds its VM
    /// on the coroutine stack, so neither is in the pool to be handed out twice.
    static VM_POOL: RefCell<fusevm::VMPool> = RefCell::new(fusevm::VMPool::new());
}

/// Enable/disable DAP debug execution. When on, `run_chunk_on` installs the
/// extension-handler seam and skips the tracing JIT (which would compile hot
/// loops and step over the `DBG_LINE` markers the debugger relies on).
///
/// Pooled VMs carry both of those settings across a `reset`, so the pool is
/// dropped here: a VM built under one mode must never be handed to the other.
pub fn set_debug_mode(on: bool) {
    DEBUG_MODE.with(|d| d.set(on));
    VM_POOL.with(|p| *p.borrow_mut() = fusevm::VMPool::new());
}

/// Register every phplang builtin + the strict numeric hook on a VM, then run it.
fn run_chunk_on(chunk: Chunk) -> Result<Value, String> {
    run_chunk_capturing(chunk, false)
}

/// [`run_chunk_on`], keeping the base frame's fusevm slots on the host when
/// `capture` is set — the main chunk's promoted globals, which request shutdown
/// still has to see after the VM is recycled.
fn run_chunk_capturing(chunk: Chunk, capture: bool) -> Result<Value, String> {
    // The hook warns (`A non-numeric value encountered`), so it needs the line
    // of the operator that delegated to it. Native arithmetic ops run no
    // builtin, so nothing else sets the warn site — hand the hook this chunk's
    // line table and let it index the op position the VM reports. Capturing it
    // per VM rather than in a thread-local keeps it correct when a generator
    // suspends mid-chunk, and the clone is dwarfed by the `Chunk` clone every
    // call already pays.
    let lines: std::sync::Arc<[u32]> = chunk.lines.as_slice().into();
    // A recycled VM already has the builtin table; only a fresh one is
    // installed onto. `VM::reset` keeps the table, the tracing-JIT flag and the
    // extension handler, which is exactly the state `install` and the mode
    // branch below would rebuild.
    let (mut vm, fresh) = VM_POOL.with(|p| {
        let mut pool = p.borrow_mut();
        let fresh = pool.is_empty();
        (pool.acquire(chunk), fresh)
    });
    if fresh {
        crate::builtins::install(&mut vm);
        if DEBUG_MODE.with(|d| d.get()) {
            vm.set_extension_handler(Box::new(|vm, id, _| {
                crate::dap::on_ext(vm, id);
            }));
        } else {
            vm.enable_tracing_jit();
        }
    }
    // Re-set every run: the hook closes over THIS chunk's line table.
    vm.set_sited_numeric_hook(std::sync::Arc::new(move |call| {
        crate::builtins::numeric_hook_sited(call, &lines)
    }));
    let outcome = vm.run();
    let err = with_host(|h| h.take_error());
    let result = match (&err, &outcome) {
        (Some(_), _) => None,
        (None, VMResult::Ok(v)) => Some(Ok(v.clone())),
        (None, VMResult::Halted) => Some(Ok(vm.stack.last().cloned().unwrap_or(Value::Undef))),
        // A builtin raising a PHP exception halts its chunk cleanly, but the
        // numeric hook has no VM handle to do that with — its only way out is
        // an error return. When it leaves an exception pending, that exception
        // is the real signal, so report the stop as a clean halt: every caller
        // (`run_body`, `run_main`, the call dispatcher) already checks for a
        // pending throw on this path, and only that path routes it to `catch`.
        (None, VMResult::Error(_)) if unwinding() => Some(Ok(Value::Undef)),
        (None, VMResult::Error(e)) => Some(Err(e.clone())),
    };
    if capture {
        let slots = vm
            .frames
            .first()
            .map(|f| f.slots.clone())
            .unwrap_or_default();
        with_host(|h| h.main_frame_slots = slots);
    }
    // Recycled only after the last read of `vm`. A generator that suspended
    // never gets here — its VM stays on the coroutine stack, which is what
    // keeps the pool from handing the same VM out twice.
    VM_POOL.with(|p| p.borrow_mut().release(vm));
    match (err, result) {
        (Some(e), _) => Err(e),
        (None, Some(r)) => r,
        (None, None) => unreachable!("result is set whenever there is no host error"),
    }
}

/// Run the top-level program chunk.
pub fn run_main(chunk: Chunk) -> Result<Value, String> {
    let r = run_chunk_capturing(chunk, true);
    // A diagnostic raised by the program's last native op has had no builtin
    // boundary to reach its handler at yet.
    drain_error_handlers();
    // A top-level `return` just ends the program; clear any leftover signal.
    with_host(|h| h.signal.take());
    // An exception that reached the top uncaught is a fatal error, displayed in
    // the PHP CLI's shape. `write_out` puts it where the reference puts it — on
    // stdout, inside any open output buffer — and the returned string is the
    // stderr log copy, which the CLI wrapper reports without re-displaying.
    if let Some(mut exc) = with_host(|h| h.pending_throw.take()) {
        // `set_exception_handler`: the handler takes the exception instead of
        // the fatal, and the run then ends normally. One it throws itself is
        // what goes uncaught. `php -r` never calls it — the reference runs that
        // code through `zend_eval_string`, which reports the exception directly.
        if let Some(handler) = with_host(|h| h.user_exception_handler()) {
            with_host(|h| {
                if let Some(g) = h.scopes.first_mut() {
                    g.internal = true;
                }
            });
            let _ = call_value(handler, vec![exc.clone()]);
            with_host(|h| h.signal.take());
            match with_host(|h| h.pending_throw.take()) {
                Some(next) => exc = next,
                None => {
                    return Ok(Value::Undef);
                }
            }
        }
        return Err(report_uncaught(exc));
    }
    r
}

/// Port of `Exception::__toString` (`Zend/zend_exceptions.c`), shared by
/// `Exception` and `Error` through the prelude's `__phplang_throwable_string`.
///
/// Walks the `previous` chain from `this` outward-in, rendering each throwable
/// as `Class: message in file:line\nStack trace:\n<trace>` (no `: message`
/// when the message is empty) and appending what was rendered so far after
/// `\n\nNext `, so the innermost exception reads first. The trace is the
/// object's own `getTraceAsString()`, `#0 {main}\n` when that is empty. A
/// user function's argument-type refusal (exactly `TypeError` or
/// `ArgumentCountError`) is extended with ` and defined`, because the file and
/// line that follow are the declaration's.
pub fn throwable_string(this: &Value) -> Result<Value, String> {
    let mut out = String::new();
    let mut exc = this.clone();
    while let Some(class) = with_host(|h| {
        h.object_class(&exc)
            // Throwable: phplang roots it at the two prelude classes.
            .filter(|c| h.class_is_a_pub(c, "Exception") || h.class_is_a_pub(c, "Error"))
    }) {
        let (message, file, line) = with_host(|h| {
            (
                h.to_str(&h.prop_get(&exc, "message")),
                h.to_str(&h.prop_get(&exc, "file")),
                h.prop_get(&exc, "line").to_int(),
            )
        });
        let trace = call_method(&class, "getTraceAsString", Some(exc.clone()), Vec::new())?;
        let trace = match trace {
            Value::Str(s) if !s.is_empty() => s.to_string(),
            _ => "#0 {main}\n".to_string(),
        };
        let message = if (class.eq_ignore_ascii_case("TypeError")
            || class.eq_ignore_ascii_case("ArgumentCountError"))
            && message.contains(", called in ")
        {
            format!("{message} and defined")
        } else {
            message
        };
        let name = display_class(&class).to_string();
        let next = if out.is_empty() { "" } else { "\n\nNext " };
        out = if message.is_empty() {
            format!("{name} in {file}:{line}\nStack trace:\n{trace}{next}{out}")
        } else {
            format!("{name}: {message} in {file}:{line}\nStack trace:\n{trace}{next}{out}")
        };
        exc = with_host(|h| h.prop_get(&exc, "previous"));
    }
    Ok(Value::str(out))
}

/// Display an exception nothing caught as the reference's fatal, on stdout
/// (inside any open output buffer), and answer the stderr log copy, which the
/// CLI wrapper reports without re-displaying.
fn report_uncaught(exc: Value) -> String {
    {
        // An uncaught `ParseError` — a syntax error in `include`d or `eval()`'d
        // code — is reported the way a syntax error in the script itself is:
        // `Parse error: <message> in <file> on line <n>`, with no trace.
        let parse = with_host(|h| {
            h.object_class(&exc)
                .is_some_and(|c| c.eq_ignore_ascii_case("ParseError"))
                .then(|| {
                    format!(
                        "{} in {} on line {}",
                        h.to_str(&h.prop_get(&exc, "message")),
                        h.to_str(&h.prop_get(&exc, "file")),
                        h.prop_get(&exc, "line").to_int()
                    )
                })
        });
        if let Some(body) = parse {
            with_host(|h| h.fatal("Parse error", &body));
            return format!("Parse error:  {body}");
        }
        // `zend_exception_error`: the text is the exception's own `__toString()`
        // — a user override included — and an exception thrown BY that
        // `__toString` is what gets reported instead.
        let class = with_host(|h| h.object_class(&exc)).unwrap_or_else(|| "Exception".to_string());
        // The engine makes the call, so a trace names its site
        // `[internal function]`.
        with_host(|h| {
            if let Some(g) = h.scopes.first_mut() {
                g.internal = true;
            }
        });
        let r = call_method(&class, "__toString", Some(exc.clone()), Vec::new());
        if let Some(inner) = with_host(|h| h.pending_throw.take()) {
            return report_uncaught(inner);
        }
        let str = r.map(|v| with_host(|h| h.to_str(&v))).unwrap_or_default();
        let body = with_host(|h| {
            let file = h.to_str(&h.prop_get(&exc, "file"));
            let line = h.prop_get(&exc, "line").to_int();
            format!("Uncaught {str}\n  thrown in {file} on line {line}")
        });
        with_host(|h| h.fatal("Fatal error", &body));
        format!("Fatal error:  {body}")
    }
}

/// Invoke a user function (or fall through to the builtin library) by name.
/// Pushes a fresh scope, binds positional parameters, runs the body chunk, and
/// returns the `return` value (or null if the body fell off the end).
/// PHP's string conversion with `__toString` honoured.
///
/// [`PhpHost::to_str`] takes `&self` and so cannot run PHP code; an object that
/// defines `__toString` therefore has to be converted *outside* the host borrow,
/// which is what this free function is for. It must not be called from inside a
/// `with_host` closure — invoking the method re-enters the host and the
/// `RefCell` would be borrowed twice.
///
/// Every other value takes the ordinary cast, and the probe for the method is a
/// single class lookup, so the non-object path costs one `with_host` round trip
/// and nothing else.
pub fn to_str_ext(v: &Value) -> String {
    let class = with_host(|h| {
        h.object_class(v)
            .filter(|c| h.class_has_method(c, "__tostring"))
    });
    let Some(class) = class else {
        return with_host(|h| h.to_str_diag(v));
    };
    match call_method(&class, "__toString", Some(v.clone()), Vec::new()) {
        Ok(r) => with_host(|h| h.to_str(&r)),
        // A throwing `__toString` leaves the exception pending for the caller's
        // dispatcher; the conversion itself yields the empty string.
        Err(_) => String::new(),
    }
}

pub fn call_function(name: &str, args: Vec<Value>) -> Result<Value, String> {
    call_function_dispatched(name, args, Dispatch::Compiled)
}

/// Whether a call was written where the COMPILER could see it, or reached the
/// function through a value at run time.
///
/// The distinction exists for the handful of names the reference compiles to an
/// opcode (see `compiles_to_an_opcode`). The compiler can only specialise a call
/// it can see: dispatched through a callback, a `$f(…)`, or `call_user_func`,
/// the same name is an ordinary internal call and its argument error carries a
/// frame like any other.
///
/// ```text
/// $ php -r 'try { strlen([1]); }                catch (Throwable $e) { echo $e->getTraceAsString(); }'
/// #0 {main}
/// $ php -r 'try { call_user_func("strlen", [1]); } catch (Throwable $e) { echo $e->getTraceAsString(); }'
/// #0 Command line code(1): strlen(Array)
/// #1 {main}
/// ```
#[derive(Clone, Copy, PartialEq)]
pub enum Dispatch {
    /// Written as a call in the source.
    Compiled,
    /// Reached through a value: `$f(…)`, a callback, `call_user_func`.
    Indirect,
}

fn call_function_dispatched(name: &str, args: Vec<Value>, how: Dispatch) -> Result<Value, String> {
    // Inline Rust FFI: the `rust { ... }` desugar emits `__rust_compile(b64,
    // line)`; compile + register the block's exported functions.
    if name == "__rust_compile" {
        let b64 = args.first().map(|v| v.to_str()).unwrap_or_default();
        return fusevm::ffi::compile_and_register(&b64).map(|_| Value::Undef);
    }
    // A `rust { ... }` block's exported functions are callable by bareword.
    // User-defined PHP functions still win (resolved below); the registry is
    // consulted before the PHP standard library so an exported name is
    // reachable, and the membership check keeps this off the hot path.
    let def = with_host(|h| h.functions.get(&name.to_ascii_lowercase()).cloned());
    if def.is_none() && fusevm::ffi::is_registered(name) {
        if let Some(r) = fusevm::ffi::try_call(name, &args) {
            return r;
        }
    }
    if let Some(def) = def {
        return invoke_with_locals(
            def.declared.as_deref().unwrap_or(name),
            Signature {
                params: &def.params,
                ret: def.ret.as_ref(),
                deprecated: def.deprecated.as_ref(),
            },
            def.chunk,
            Vec::new(),
            args,
            Vec::new(),
            def.is_generator,
            &def.locals,
        );
    }
    // A library function names itself in lower case in every message and trace
    // frame, however the call spelled it: `STRLEN([])` reports `strlen()`.
    call_library_throwing(&crate::argsig::lower_name(name), args, how)
}

/// Call a standard-library function, turning a tagged argument error into the
/// PHP exception it stands for (see [`crate::builtins::throws`]).
///
/// This wrapper is the caller rather than part of `call_library` itself because
/// it needs the argument list AFTER the call, to render the trace frame —
/// `call_library` borrows it, so the owner has to be the one that reacts.
/// Raise a tagged library error through the ordinary internal-throw path, so it
/// carries the same trace frame a failure inside the function would.
fn throw_from_internal_typed(name: &str, args: &[Value], e: String) -> Result<Value, String> {
    match crate::builtins::untag_throw(&e) {
        Some((class, message)) => throw_from_internal(name, args, class, message),
        None => Err(e),
    }
}

/// Whether the reference COMPILES this call to an opcode instead of a call, so
/// the argument error it raises has no frame of its own and the trace starts at
/// the caller.
///
/// The specialisation is arity-exact: `strlen($s)` becomes `ZEND_STRLEN`, but
/// `strlen($s, 1)` is an ordinary call whose `ArgumentCountError` IS framed, so
/// the count is part of the test. Measured for each name at both arities:
///
/// ```text
/// $ php -r 'try { strlen([1]); }   catch (Throwable $e) { echo $e->getTraceAsString(); }'
/// #0 {main}
/// $ php -r 'try { count([1], "x"); } catch (Throwable $e) { echo $e->getTraceAsString(); }'
/// #0 Command line code(1): count(Array, 'x')
/// #1 {main}
/// ```
///
/// The rest of PHP's special-cased names (`chr`, `ord`, `defined`, `in_array`,
/// `array_slice`, `sprintf`, `intval`) specialise only for a literal argument,
/// so a runtime call to one is compiled normally and frames its error; all seven
/// were measured framed and are deliberately absent.
fn compiles_to_an_opcode(name: &str, argc: usize) -> bool {
    match name {
        "strlen" | "count" | "sizeof" | "get_class" => argc == 1,
        "array_key_exists" => argc == 2,
        _ => false,
    }
}

/// Raise a tagged library error from the CALLER's frame: the exception is built
/// and left pending without pushing a frame for the library function, which is
/// what the reference produces for a call it compiled to an opcode.
fn throw_frameless_typed(e: &str) -> Result<Value, String> {
    match crate::builtins::untag_throw(e) {
        Some((class, message)) => {
            let exc = new_object(class, vec![Value::str(message.to_string())])?;
            set_pending_throw(exc);
            Ok(Value::Undef)
        }
        None => Err(e.to_string()),
    }
}

/// Whether a library function runs a PHP callback, and so occupies a frame of its
/// own in a trace captured from inside that callback.
///
/// Every name here was measured against the reference by throwing out of the
/// callback and reading the trace back. `call_user_func` and
/// `call_user_func_array` are deliberately ABSENT: the reference invokes their
/// callee from the caller's own frame, so neither appears in the trace and the
/// callee reports the call site rather than `[internal function]`.
///
/// The list is the set of callback-taking functions this engine implements —
/// every `host::call_value` reached from `builtins::call_library` — so a new one
/// has to be added here along with its dispatch arm.
/// The library functions that run a PHP callback, named as a slice so the
/// invariant `call_library_throwing` leans on can be TESTED: none of these
/// declares a parameter the argument parser converts in the slot, so the frame
/// they push may record its arguments as written (see
/// `crate::argtypes::converts_in_place`, and the test beside it).
pub(crate) const CALLS_BACK: &[&str] = &[
    "array_map",
    "array_filter",
    "array_reduce",
    "array_walk",
    "array_walk_recursive",
    "array_find",
    "array_find_key",
    "array_any",
    "array_all",
    "array_udiff",
    "array_uintersect",
    "array_diff_ukey",
    "array_intersect_ukey",
    "array_diff_uassoc",
    "array_intersect_uassoc",
    "array_udiff_assoc",
    "array_uintersect_assoc",
    "array_udiff_uassoc",
    "array_uintersect_uassoc",
    "ob_flush",
    "ob_clean",
    "ob_end_flush",
    "ob_end_clean",
    "ob_get_flush",
    "ob_get_clean",
    "usort",
    "uasort",
    "uksort",
    "preg_replace_callback",
    "preg_replace_callback_array",
    "iterator_apply",
    "iterator_to_array",
    "iterator_count",
];

fn calls_back(name: &str) -> bool {
    CALLS_BACK.contains(&name)
}

fn call_library_throwing(name: &str, args: Vec<Value>, how: Dispatch) -> Result<Value, String> {
    // PHP 8 refuses an argument whose type the parameter does not accept, before
    // the function runs. The check is by DECLARED TYPE rather than per function
    // (see `crate::argtypes`), and it goes through the same error path as any
    // other library failure so the refusal carries the call's trace frame.
    // A call the reference compiles to an opcode raises its argument error from
    // the CALLER's frame, because no call was ever made (see
    // `compiles_to_an_opcode`).
    let opcode = how == Dispatch::Compiled && compiles_to_an_opcode(name, args.len());
    // The COUNT is checked before the types, and a `callable` parameter is
    // judged in its own position among them — see `crate::argsig::check_args`.
    static NOCHECK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let nocheck = *NOCHECK.get_or_init(|| std::env::var_os("PHPLANG_NOCHECK").is_some());
    let checked = if nocheck {
        Ok(())
    } else {
        crate::argsig::check_args(name, &args)
    };
    if let Err(e) = checked {
        return if opcode {
            throw_frameless_typed(&e)
        } else {
            // The frame renders the arguments the parser had already CONVERTED
            // when it stopped (see `crate::argtypes::trace_args`). An arity
            // refusal is raised before any argument is parsed, so that one
            // reports them exactly as written: `strtolower(5, 6)`.
            let arity = crate::builtins::untag_throw(&e)
                .is_some_and(|(class, _)| class == "ArgumentCountError");
            let targs = if arity {
                std::borrow::Cow::Borrowed(&args[..])
            } else {
                crate::argtypes::trace_args(name, &args)
            };
            throw_from_internal_typed(name, &targs, e)
        };
    }
    // A library function that runs a PHP callback IS a frame in the reference's
    // trace, and the callback's own frame reports `[internal function]` as its
    // call site. Pushed only for the functions that can call back, so the frame
    // stack — and everything that resolves a variable through it — is untouched
    // for the rest of the library.
    let framed = calls_back(name);
    if framed {
        with_host(|h| {
            let line = h.cur_frame_line();
            let argsarr = h.new_array();
            for a in &args {
                h.arr_push_auto(&argsarr, a.clone());
            }
            h.push_internal_frame(name, line, argsarr);
        });
    }
    let out = crate::builtins::call_library(name, &args);
    if framed {
        with_host(PhpHost::pop_internal_frame);
    }
    match out {
        Err(e) => {
            // Every frame below renders the arguments as the parser left them.
            let targs = crate::argtypes::trace_args(name, &args);
            // A frameless throw is raised from the caller's own frame: no scope
            // is pushed, so the trace starts where the call was written.
            if let Some((class, message)) = crate::builtins::untag_bare_throw(&e) {
                let exc = new_object(class, vec![Value::str(message.to_string())])?;
                set_pending_throw(exc);
                return Ok(Value::Undef);
            }
            // An engine-level fatal, not a Throwable: displayed with the same
            // trace a throw would carry but never routed through `catch`, and
            // the program stops.
            if let Some(message) = crate::builtins::untag_fatal(&e) {
                return Err(fatal_from_internal(name, &targs, message));
            }
            if let Some((class, code, message)) = crate::builtins::untag_throw_code(&e) {
                return throw_from_internal_args(
                    name,
                    &targs,
                    class,
                    vec![Value::str(message.to_string()), Value::int(code)],
                );
            }
            if opcode {
                return throw_frameless_typed(&e);
            }
            match crate::builtins::untag_throw(&e) {
                Some((class, message)) => throw_from_internal(name, &targs, class, message),
                None => Err(e),
            }
        }
        ok => ok,
    }
}

/// `call_function` with PHP 8.0 named arguments. User functions bind by parameter
/// name; a builtin (no name metadata) falls back to positional binding with the
/// named values appended in call order.
pub fn call_function_named(
    name: &str,
    args: Vec<Value>,
    named: Vec<(String, Value)>,
) -> Result<Value, String> {
    let def = with_host(|h| h.functions.get(&name.to_ascii_lowercase()).cloned());
    if let Some(def) = def {
        return invoke_with_locals(
            def.declared.as_deref().unwrap_or(name),
            Signature {
                params: &def.params,
                ret: def.ret.as_ref(),
                deprecated: def.deprecated.as_ref(),
            },
            def.chunk,
            Vec::new(),
            args,
            named,
            def.is_generator,
            &def.locals,
        );
    }
    let lname = crate::argsig::lower_name(name);
    let name = lname.as_ref();
    // A builtin binds by name too — the table in `crate::argsig` is what says
    // which parameter a name means, which slot it fills, and what belongs in the
    // slots a name jumped over. Only a name the table does not describe (an FFI
    // export, `__cast_array`) still falls back to appending positionally.
    if crate::argsig::sig_of(name).is_some() {
        return match crate::argsig::bind_named(name, args, named) {
            // The two the reference refuses at SEND time carry no frame, so
            // they need no argument list either.
            Err(e) => match crate::builtins::untag_bare_throw(&e) {
                Some((class, message)) => {
                    let exc = new_object(class, vec![Value::str(message.to_string())])?;
                    set_pending_throw(exc);
                    Ok(Value::Undef)
                }
                None => Err(e),
            },
            Ok(bound) => {
                // A name a variadic could not place is the LAST refusal, after
                // the types: `sprintf([], x: 1)` reports the array in #1.
                let mut shown = bound.shown;
                let refusal = bound.refusal.or_else(|| {
                    bound.unplaced_name.then(|| {
                        // The parameters parsed ahead of the variadic have been
                        // converted by then, and the trace shows them so:
                        // `sprintf(1, x: 1)` prints `sprintf('1', x: 1)`.
                        let head = crate::argsig::zpp_head_len(name).min(bound.args.len());
                        let conv = crate::argtypes::trace_args(name, &bound.args[..head]);
                        for (slot, v) in shown.iter_mut().zip(conv.iter()) {
                            slot.1 = v.clone();
                        }
                        crate::argsig::refuse_unplaced(name, &bound.args)
                    })
                });
                match refusal {
                    Some(e) => throw_from_internal_named(name, &shown, e),
                    None => call_function(name, bound.args),
                }
            }
        };
    }
    let mut all = args;
    all.extend(named.into_iter().map(|(_, v)| v));
    call_function(name, all)
}

/// Raise a tagged refusal with a trace frame whose arguments are the ones the
/// call BOUND: every slot it reached, a slot it jumped over rendered `NULL`, and
/// a name a variadic could not place rendered `name: value` — which is how the
/// reference prints them.
///
/// ```text
/// $ php -r 'function f(){ count(mode: "ab"); } f();'
/// #0 Command line code(1): count(NULL, 'ab')
/// ```
fn throw_from_internal_named(
    func: &str,
    shown: &[(Option<String>, Value)],
    e: String,
) -> Result<Value, String> {
    let Some((class, message)) = crate::builtins::untag_throw(&e) else {
        return Err(e);
    };
    let (class, message) = (class.to_string(), message.to_string());
    with_host(|h| {
        let line = h.cur_frame_line();
        h.scopes.push(Scope {
            name: Some(func.to_string()),
            line,
            internal: true,
            ..Scope::default()
        });
        let argsarr = h.new_array();
        for (name, v) in shown {
            match name {
                Some(n) => h.arr_set_key(&argsarr, &Value::str(n.clone()), v.clone()),
                None => h.arr_push_auto(&argsarr, v.clone()),
            }
        }
        h.set_var("@args", argsarr);
    });
    let exc = new_object(&class, vec![Value::str(message)])?;
    with_host(|h| {
        h.scopes.pop();
    });
    set_pending_throw(exc);
    Ok(Value::Undef)
}

/// Run a user-defined function or closure body: push a call frame named `frame`,
/// pre-bind `pre` (a closure's captured `(name, value)` pairs, empty for a plain
/// function), bind `args` to `params` (variadic collection, then default chunks
/// for omitted parameters), run `body`, and return the `return` value (or null if
/// the body fell off the end). Default chunks run OUTSIDE the binding `with_host`
/// closure because `run_chunk_on` itself borrows the thread-local host.
#[allow(clippy::too_many_arguments)]
/// Apply each declared parameter type to the argument bound to it, returning the
/// values the callee should actually see.
///
/// Runs BEFORE any binding, because a `TypeError` here means the call never
/// happens — no scope is pushed and no default is evaluated. A parameter with no
/// type, or one whose type is not an enforced scalar, passes its argument through
/// untouched, which is what every parameter did before types were carried at all.
/// A frame name as it should READ in a diagnostic.
///
/// `resolve_method` keys classes by their lowercased name and hands that back, so
/// a frame arrives as `c::m` where PHP writes `C::m`. The declared spelling is
/// recovered here rather than at the call sites, which use the frame for scope
/// lookups that must keep matching case-insensitively.
fn display_frame(frame: &str) -> String {
    match frame.split_once("::") {
        Some((cls, rest)) => {
            let shown = with_host(|h| h.class_display_name(&cls.to_ascii_lowercase()));
            format!("{shown}::{rest}")
        }
        None => frame.to_string(),
    }
}

/// `Ok(None)` means a `TypeError` was raised and is pending: the call must not
/// happen, and the caller returns null while the dispatcher unwinds it.
type ArgCheck = Result<Option<(Vec<Value>, Vec<(String, Value)>)>, String>;

/// The call-shape errors PHP raises before a body runs: a named argument that
/// names nothing, one that collides with a positional, and a required parameter
/// left unfilled.
///
/// Separate from [`check_arg_types`], which returns early when no parameter is
/// typed — these apply to every call, typed or not. `Ok(false)` means an error
/// is pending and the caller must not run the body.
fn check_call_shape(
    frame: &str,
    params: &[Param],
    args: &[Value],
    named: &[(String, Value)],
) -> Result<bool, String> {
    let called_with: Vec<Value> = args.to_vec();
    let has_variadic = params.iter().any(|p| p.variadic);
    for (n, _) in named {
        match params.iter().position(|p| !p.variadic && p.name == *n) {
            // Naming a parameter a positional argument already filled is an
            // error, not a silent overwrite — PHP will not let one call bind the
            // same parameter twice.
            Some(i) if i < args.len() => {
                throw_from_internal(
                    frame,
                    &called_with,
                    "Error",
                    &format!("Named parameter ${n} overwrites previous argument"),
                )?;
                return Ok(false);
            }
            Some(_) => {}
            // A variadic collects names it cannot place; without one there is
            // nowhere for the name to go and dropping it was silent.
            None if !has_variadic => {
                throw_from_internal(
                    frame,
                    &called_with,
                    "Error",
                    &format!("Unknown named parameter ${n}"),
                )?;
                return Ok(false);
            }
            None => {}
        }
    }
    // A method of a prelude class stands in for an INTERNAL one, whose count
    // check is `zend_wrong_parameters_count_error`: it refuses too many
    // arguments as well as too few, and words both as `expects`.
    let required = params
        .iter()
        .filter(|p| !p.variadic && p.default.is_none())
        .count();
    if named.is_empty() && is_prelude_frame(frame) {
        let declared = params.iter().filter(|p| !p.variadic).count();
        let passed = args.len();
        if passed < required || (!has_variadic && passed > declared) {
            let (bound, n) = if required == declared && !has_variadic {
                ("exactly", required)
            } else if passed < required {
                ("at least", required)
            } else {
                ("at most", declared)
            };
            let s = if n == 1 { "" } else { "s" };
            throw_from_callee(
                frame,
                &called_with,
                "ArgumentCountError",
                &format!(
                    "{}() expects {bound} {n} argument{s}, {passed} given",
                    callee_shown(frame)
                ),
            )?;
            return Ok(false);
        }
    }
    // A parameter is filled positionally, filled by name, variadic, or has a
    // default. Anything else leaves the call short.
    let unfilled = params.iter().enumerate().find(|(i, p)| {
        !p.variadic
            && p.default.is_none()
            && *i >= args.len()
            && !named.iter().any(|(n, _)| *n == p.name)
    });
    let Some((i, p)) = unfilled else {
        return Ok(true);
    };
    let shown = callee_shown(frame);
    // A call that left a HOLE is reported by the parameter it left empty, not by
    // a count: the reference has no shortfall to report when the arguments
    // reached past it.
    //
    // ```text
    // $ php -r 'function f($a,$b,$c){} f(1);'       -> Too few arguments to function f(), 1 passed …
    // $ php -r 'function f($a,$b,$c){} f(1, c: 3);' -> f(): Argument #2 ($b) not passed
    // ```
    // …and only for a slot the call reached PAST. One left short at the end is
    // an ordinary shortfall, however it was filled:
    //
    // ```text
    // $ php -r 'function f($a,$b,$c){} f(1, b: 2);' -> Too few arguments to function f(), 2 passed …
    // ```
    let reached = params
        .iter()
        .enumerate()
        .filter(|(_, p)| named.iter().any(|(n, _)| *n == p.name))
        .map(|(j, _)| j + 1)
        .chain(std::iter::once(args.len()))
        .max()
        .unwrap_or(0);
    // The frame shows the arguments where they landed: a named one in its
    // parameter's slot, and NULL in each slot left empty before it —
    // `f(1, c: 3)` is traced as `f(1, NULL, 3)`.
    let placed: Vec<Value> = (0..reached)
        .map(|j| match args.get(j) {
            Some(v) => v.clone(),
            None => named
                .iter()
                .find(|(n, _)| *n == params[j].name)
                .map_or(Value::Undef, |(_, v)| v.clone()),
        })
        .collect();
    if i < reached {
        let argno = i + 1;
        throw_at_param(
            frame,
            &placed,
            "ArgumentCountError",
            &format!("{shown}(): Argument #{argno} (${}) not passed", p.name),
            p.line,
        )?;
        return Ok(false);
    }
    // "exactly" when every DECLARED parameter is required, "at least" when the
    // function also takes optional ones. A variadic tail is not a parameter for
    // this count — `function f($a, ...$r)` called with none is `exactly 1`.
    let declared = params.iter().filter(|p| !p.variadic).count();
    let bound = if required == declared {
        "exactly"
    } else {
        "at least"
    };
    let passed = args.len() + named.len();
    let (file, line) = with_host(|h| (h.current_file().to_string(), h.cur_frame_line()));
    throw_at_param(
        frame,
        &placed,
        "ArgumentCountError",
        &format!(
            "Too few arguments to function {shown}(), {passed} passed in {file} \
             on line {line} and {bound} {required} expected"
        ),
        p.line,
    )?;
    Ok(false)
}

fn check_arg_types(
    frame: &str,
    params: &[Param],
    args: Vec<Value>,
    named: Vec<(String, Value)>,
) -> ArgCheck {
    if params.iter().all(|p| p.ty.is_none()) {
        return Ok(Some((args, named)));
    }
    // The trace frame prints the arguments AS CALLED, so it is built from the
    // originals rather than from whatever survived coercion.
    let scope = TypeScope::of_frame(frame, None);
    let called_with = args.clone();
    let mut out = Vec::with_capacity(args.len());
    for (i, a) in args.into_iter().enumerate() {
        // Past the last declared parameter every remaining argument belongs to the
        // variadic one, and so is checked against ITS type.
        let p = params
            .get(i)
            .or_else(|| params.last().filter(|p| p.variadic));
        match coerce_arg(p, a, &scope)? {
            Ok(v) => out.push(v),
            Err(given) => {
                arg_type_error(frame, &called_with, p, i + 1, &given)?;
                return Ok(None);
            }
        }
    }
    let mut nout = Vec::with_capacity(named.len());
    for (n, v) in named {
        let pos = params.iter().position(|p| !p.variadic && p.name == n);
        let p = pos.map(|i| &params[i]);
        match coerce_arg(p, v, &scope)? {
            Ok(v) => nout.push((n, v)),
            Err(given) => {
                arg_type_error(frame, &called_with, p, pos.map_or(0, |i| i + 1), &given)?;
                return Ok(None);
            }
        }
    }
    Ok(Some((out, nout)))
}

/// One argument against one parameter's declared type. The inner `Result` is the
/// TYPE verdict — `Err(name)` carrying the type the value actually had — while the
/// outer one is reserved for a host failure.
#[allow(clippy::type_complexity)]
fn coerce_arg(
    p: Option<&Param>,
    v: Value,
    scope: &TypeScope,
) -> Result<Result<Value, String>, String> {
    let Some(p) = p else { return Ok(Ok(v)) };
    let Some(ty) = &p.ty else { return Ok(Ok(v)) };
    let ty = ty.clone();
    // A by-reference parameter is checked and coerced like any other, and the
    // converted value is written back THROUGH the reference before the body runs:
    // `function f(int &$x)` called with a `$v` holding `"5"` leaves `$v` as
    // `int(5)` even if the body never touches `$x`. An argument that arrives as a
    // reference cell is therefore rewritten in place, so the caller's variable and
    // the parameter still name one storage location.
    if p.by_ref {
        let line = p.line;
        let (slot, cur) = with_host(|h| {
            let slot = h.ref_slot_of_value(&v);
            let cur = match slot {
                Some(s) => h.ref_cell_value(s),
                None => v.clone(),
            };
            (slot, cur)
        });
        let saved = warn_line();
        set_warn_line(line);
        let r = verify_type(cur, &ty, scope);
        set_warn_line(saved);
        return Ok(with_host(|h| {
            r.map(|converted| match slot {
                Some(s) => {
                    h.ref_cell_set(s, converted);
                    // The HANDLE is what the binder needs: it aliases the
                    // parameter to the cell just rewritten.
                    v
                }
                // No cell: the caller writes the parameter's final value back
                // itself when the call returns, so converting the value is all
                // this side has to do.
                None => converted,
            })
        }));
    }
    // A lossy implicit conversion is reported against the PARAMETER's declaration,
    // not against the call — `f(int $x)` on line 2 called from line 9 names line 2
    // — so the diagnostic line is moved for the duration of the check and put back
    // after it, leaving the caller's line intact for anything the body warns about.
    let saved = warn_line();
    set_warn_line(p.line);
    let r = verify_type(v, &ty, scope);
    set_warn_line(saved);
    Ok(r)
}

/// Throw a refusal of a user function's arguments. The reference raises it
/// from the callee's receive for that parameter, so the exception's `line` is
/// where the parameter is DECLARED while its trace still shows the call:
///
/// ```text
/// $ php -r 'function f(
/// $a)
/// {}
/// f();'   -> Too few arguments to function f(), 0 passed in … on line 4 … in Command line code:2
/// ```
fn throw_at_param(
    frame: &str,
    called_with: &[Value],
    class: &str,
    msg: &str,
    decl_line: u32,
) -> Result<Value, String> {
    let r = throw_from_callee(frame, called_with, class, msg);
    // A method of a PHP-written prelude class stands in for an internal one,
    // which has no declaration to point at: the caller's line stays.
    if is_prelude_frame(frame) {
        return r;
    }
    with_host(|h| {
        if let Some(exc) = h.pending_throw.clone() {
            h.prop_set(&exc, "line", Value::int(decl_line as i64));
        }
    });
    r
}

/// Whether `frame` is a method of a PHP-written prelude class, which stands in
/// for an internal one.
fn is_prelude_frame(frame: &str) -> bool {
    frame
        .split_once("::")
        .is_some_and(|(c, _)| crate::prelude_type(&c.to_ascii_lowercase()).is_some())
}

/// Raise the `TypeError` for an argument that did not satisfy its declared type,
/// from a frame naming the callee — so the trace reads `#0 file(line): f('abc')`
/// exactly as it does when the reference rejects the same call.
fn arg_type_error(
    frame: &str,
    called_with: &[Value],
    p: Option<&Param>,
    pos: usize,
    given: &str,
) -> Result<Value, String> {
    let (name, ty) = match p {
        // A variadic parameter has no name in the message: PHP writes
        // `Argument #2 must be of type int`, with no `($xs)` after the number.
        Some(p) if p.variadic => (String::new(), p.ty.clone()),
        Some(p) => (format!(" (${})", p.name), p.ty.clone()),
        None => (String::new(), None),
    };
    let rendered = ty
        .map(|t| render_declared(&t, frame, None))
        .unwrap_or_default();
    let (file, line) = with_host(|h| (h.current_file().to_string(), h.cur_frame_line()));
    let shown = callee_shown(frame);
    // An internal function names no call site: it has no declaration either.
    let site = if is_prelude_frame(frame) {
        String::new()
    } else {
        format!(", called in {file} on line {line}")
    };
    let msg = &format!(
        "{shown}(): Argument #{pos}{name} must be of type {rendered}, {given} given{site}"
    );
    match p {
        Some(p) => throw_at_param(frame, called_with, "TypeError", msg, p.line),
        None => throw_from_callee(frame, called_with, "TypeError", msg),
    }
}

/// Apply a declared return type to the value a body produced. Like the argument
/// check, a failure raises a pending `TypeError` and yields null.
fn check_ret_type(
    frame: &str,
    ret: Option<&TypeHint>,
    called_with: &[Value],
    v: Value,
    static_class: Option<String>,
) -> Result<Value, String> {
    let Some(ty) = ret else { return Ok(v) };
    // `void` and `never` are refused while compiling; there is nothing left
    // to check here.
    if ty
        .parts
        .iter()
        .any(|p| p.eq_ignore_ascii_case("void") || p.eq_ignore_ascii_case("never"))
    {
        return Ok(v);
    }
    let ty = ty.clone();
    let scope = TypeScope::of_frame(frame, static_class);
    let scope_static = scope
        .static_class
        .clone()
        .or_else(|| scope.self_class.clone());
    match verify_type(v, &ty, &scope) {
        Ok(nv) => Ok(nv),
        // A return diagnostic names no call site — the `return` IS the site — but
        // the trace still enters the function the value was returned from.
        Err(given) => throw_from_callee(
            frame,
            called_with,
            "TypeError",
            &format!(
                "{}(): Return value must be of type {}, {given} returned",
                callee_shown(frame),
                render_declared(&ty, frame, scope_static.as_deref())
            ),
        ),
    }
}

/// A callee's declared signature: the parameters to bind on the way in and the
/// return type to check on the way out. The two travel together because every
/// caller of [`invoke`] has both and `invoke` reads them at opposite ends of the
/// same call.
struct Signature<'a> {
    params: &'a [Param],
    ret: Option<&'a TypeHint>,
    /// The callee's `#[\Deprecated]` attribute.
    deprecated: Option<&'a crate::ast::Deprecation>,
}

fn invoke(
    frame: &str,
    sig: Signature<'_>,
    body: Chunk,
    pre: Vec<(String, Value)>,
    args: Vec<Value>,
    named: Vec<(String, Value)>,
    is_generator: bool,
) -> Result<Value, String> {
    invoke_with_locals(frame, sig, body, pre, args, named, is_generator, &[])
}

/// [`invoke`] plus the callee's slot order: the frame reserves those slots, in
/// that order, before anything is bound, so an index the body was compiled with
/// names the same variable at runtime. Binding still goes through `set_var`,
/// which finds the reserved slot by name.
#[allow(clippy::too_many_arguments)]
fn invoke_with_locals(
    frame: &str,
    sig: Signature<'_>,
    body: Chunk,
    pre: Vec<(String, Value)>,
    args: Vec<Value>,
    named: Vec<(String, Value)>,
    is_generator: bool,
    locals: &[String],
) -> Result<Value, String> {
    let Signature {
        params,
        ret,
        deprecated,
    } = sig;
    // `ZEND_ACC_DEPRECATED`: the call raises the deprecation before the callee
    // binds anything, from the caller's line — and stops if a handler threw.
    if let Some(d) = deprecated {
        raise_deprecated_call(frame, d);
        // A handler runs NOW, before the callee is entered, as the reference's
        // `zend_deprecated_function` calls it.
        drain_error_handlers();
        if with_host(|h| h.pending_throw.is_some()) {
            return Ok(Value::Undef);
        }
    }
    // What a refusal raised for this call puts in its frame: the bound `$this`
    // (`A->m`) and a closure's site (`{closure:f.php:3}`).
    let ctx = CalleeCtx {
        this: pre
            .iter()
            .find(|(k, _)| k == "this")
            .map(|(_, v)| v.clone()),
        site: with_host(|h| h.pending_closure_site.clone()),
    };
    let checked = with_callee(&ctx, || check_call_shape(frame, params, &args, &named));
    if !checked? {
        // The call was rejected before it began; the dispatcher unwinds the
        // pending error.
        return Ok(Value::Undef);
    }
    let typed = with_callee(&ctx, || check_arg_types(frame, params, args, named));
    let Some((args, named)) = typed? else {
        // The call was rejected before it began: a `TypeError` is pending and the
        // dispatcher will unwind it.
        return Ok(Value::Undef);
    };
    // The trace of a return-type failure prints the arguments the call was made
    // with, so they are kept only when there is such a type to fail.
    let ret_args: Vec<Value> = match ret {
        Some(_) => args.clone(),
        None => Vec::new(),
    };
    // Which parameter positions a positional or named argument (or a null-fill for
    // a no-default omitted param) already bound — so the default pass below runs
    // only the chunks that are actually needed. Computed inside the binding closure.
    // Code compiled from another file names that file; the main script's
    // chunks carry no source, and their frames fall back to the script name.
    let file = (!body.source.is_empty()).then(|| std::sync::Arc::from(body.source.as_str()));
    let bound = with_host(|h| {
        let scope = Scope {
            name: Some(frame.to_string()),
            static_class: h.lsb_take(),
            closure_site: h.closure_site_take(),
            transparent: std::mem::take(&mut h.pending_transparent),
            ..Scope::default()
        };
        let scope = Scope { file, ..scope };
        h.scopes.push(scope);
        h.seed_slots(locals);
        // Captured bindings first, then parameters (a parameter of the same name
        // as a capture shadows it, as PHP does).
        for (k, v) in pre {
            // A `use (&$v)` capture arrives as a handle to the enclosing
            // variable's cell: bind the name to that cell rather than storing
            // the handle, and the two are one variable from here on.
            match v {
                Value::Obj(id) => match h.objs.get(id as usize) {
                    Some(PhpObj::Ref { slot }) => {
                        let slot = *slot;
                        h.bind_ref_slot(&k, slot);
                    }
                    _ => h.set_var(&k, v),
                },
                _ => h.set_var(&k, v),
            }
        }
        let mut bound = vec![false; params.len()];
        // Positional binding (a variadic `...$rest` collects the rest positionally).
        let mut ai = 0;
        for (i, p) in params.iter().enumerate() {
            if p.variadic {
                let arr = h.new_array();
                while ai < args.len() {
                    h.arr_push_auto(&arr, args[ai].clone());
                    ai += 1;
                }
                h.set_var(&p.name, arr);
                bound[i] = true;
            } else if ai < args.len() {
                // A by-value parameter takes a copy of an array argument; a
                // by-reference one must see the caller's array itself.
                match (p.by_ref, &args[ai]) {
                    // A caller that supplied an explicit reference cell (a
                    // builtin passing an array element to a callback, say) binds
                    // the parameter to that cell, so scalar writes are visible to
                    // the caller too — not just in-place array mutation.
                    (true, Value::Obj(id))
                        if matches!(h.objs.get(*id as usize), Some(PhpObj::Ref { .. })) =>
                    {
                        let Some(PhpObj::Ref { slot }) = h.objs.get(*id as usize) else {
                            unreachable!("guarded by the match arm above")
                        };
                        let slot = *slot;
                        h.bind_ref_slot(&p.name, slot);
                    }
                    (true, a) => h.set_var(&p.name, a.clone()),
                    (false, a) => {
                        // A by-value parameter never sees a reference cell: read
                        // through it so a builtin that offers an element by
                        // reference still works with a plain `function ($v)`.
                        let a = match a {
                            Value::Obj(id) => match h.objs.get(*id as usize) {
                                Some(PhpObj::Ref { slot }) => {
                                    let slot = *slot;
                                    h.ref_cell_value(slot)
                                }
                                _ => a.clone(),
                            },
                            _ => a.clone(),
                        };
                        let copied = h.copy_on_assign(a);
                        h.set_var(&p.name, copied);
                    }
                }
                bound[i] = true;
                ai += 1;
            }
        }
        // Named binding: match each `name: value` to the parameter of that name; an
        // unmatched name lands in the variadic parameter's array under a string key
        // (as PHP collects extra named args), or is dropped if there is no variadic.
        for (n, v) in &named {
            if let Some(i) = params.iter().position(|p| !p.variadic && p.name == *n) {
                let v = if params[i].by_ref {
                    v.clone()
                } else {
                    h.copy_on_assign(v.clone())
                };
                h.set_var(&params[i].name, v);
                bound[i] = true;
            } else if let Some(vi) = params.iter().position(|p| p.variadic) {
                let arr = h.get_var(&params[vi].name);
                h.arr_set_key(&arr, &Value::str(n.clone()), v.clone());
            }
        }
        // A parameter with neither an argument nor a default reads as null.
        for (i, p) in params.iter().enumerate() {
            if !bound[i] && !p.variadic && p.default.is_none() {
                h.set_var(&p.name, Value::Undef);
                bound[i] = true;
            }
        }
        // The call's argument list as `func_get_args()` reports it, in the hidden
        // `@args`/`@argnames` pair. Since PHP 7 the reference reports a declared
        // parameter's CURRENT value, not the value the caller passed — `function
        // f($a) { $a = 99; return func_get_args(); }` called as `f(1)` answers
        // `[99]`. So a position covered by a declared parameter stashes that
        // parameter's NAME and the read goes through the variable; only an
        // argument PAST the declared parameters is stored by value, which is why
        // mutating a variadic's array does not change what is reported.
        //
        // The count is the number of arguments the call supplied. A named
        // argument counts at its PARAMETER's position, so `f(9, c: 7)` on
        // `f($a=1,$b=2,$c=3)` reports three — the skipped `$b` reported at its
        // default, exactly as reading the parameter gives.
        let n_decl = params.iter().filter(|p| !p.variadic).count();
        let mut argc = args.len();
        for (n, _) in &named {
            if let Some(i) = params.iter().position(|p| !p.variadic && p.name == *n) {
                argc = argc.max(i + 1);
            }
        }
        let argsarr = h.new_array();
        let namesarr = h.new_array();
        for i in 0..argc {
            // A position covered by a declared parameter records the NAME and no
            // value; one past them records the value and no name.
            let declared = params.get(i).filter(|_| i < n_decl);
            let (name, value) = match declared {
                Some(p) => (Value::str(p.name.clone()), Value::Undef),
                None => (Value::Undef, args.get(i).cloned().unwrap_or(Value::Undef)),
            };
            h.arr_push_auto(&namesarr, name);
            h.arr_push_auto(&argsarr, value);
        }
        h.set_var("@args", argsarr);
        h.set_var("@argnames", namesarr);
        bound
    });
    // Evaluate defaults for the still-unbound parameters, left to right, so a
    // default may reference an earlier parameter already bound in this frame.
    for (i, p) in params.iter().enumerate() {
        if p.variadic || bound[i] {
            continue;
        }
        if let Some(chunk) = &p.default {
            match run_chunk_on(chunk.clone()) {
                Ok(v) => with_host(|h| h.set_var(&p.name, v)),
                Err(e) => {
                    with_host(|h| {
                        h.scopes.pop();
                    });
                    return Err(e);
                }
            }
        }
    }
    // A generator function does not run its body on call: it hands the fully-bound
    // call frame to a suspended coroutine and returns a Generator handle. The frame
    // is pulled off the active stack and lives in the generator until it finishes.
    if is_generator {
        let frame_scope = with_host(|h| h.scopes.pop().expect("generator call frame"));
        return Ok(make_generator(body, frame_scope));
    }
    let r = run_chunk_on(body);
    let sig = with_host(|h| {
        // Capture by-reference parameters' final values (read from the still-open
        // callee frame) so the caller can write them back after the call. Recorded
        // for EVERY call, so a callee with no by-reference parameter clears what
        // the previous one left rather than letting a caller read it back.
        let vals = params
            .iter()
            .map(|p| {
                if p.by_ref {
                    h.get_var(&p.name)
                } else {
                    Value::Undef
                }
            })
            .collect();
        h.byref_out_set(vals, params.iter().map(|p| p.by_ref).collect());
        let static_class = h.scopes.pop().and_then(|s| s.static_class);
        (h.signal.take(), static_class)
    });
    let (sig, static_class) = sig;
    // A pending exception (set by `throw`, kept in its own field) survives the
    // scope pop and takes precedence — the caller's dispatcher checks
    // `has_pending_throw` and re-halts to keep it bubbling.
    match sig {
        Some(Signal::Return(v)) => with_callee(&ctx, || {
            check_ret_type(frame, ret, &ret_args, v, static_class)
        }),
        // A `break`/`continue` that escapes a function body has no loop to
        // target; PHP treats it as falling off the end (null result). Falling off
        // the end is NOT checked against the return type: a `void` function does
        // it by design, and a typed one that reaches the end without returning is
        // a separate diagnostic this engine does not raise.
        Some(Signal::Break(_)) | Some(Signal::Continue(_)) | None => r.map(|_| Value::Undef),
    }
}

/// Run a closure body: bind its captures (a bound `$this` overriding any captured
/// one) and set the frame's class scope so private/protected access matches the
/// bound scope, then dispatch (as a generator, if its body yields).
fn invoke_closure(
    cc: ClosureCall,
    args: Vec<Value>,
    named: Vec<(String, Value)>,
) -> Result<Value, String> {
    // Encode the private-access scope in the frame name (`Scope::{closure}`), which
    // `current_class_ctx` reads back for visibility checks.
    let frame = match &cc.scope {
        Some(s) => format!("{s}::{{closure}}"),
        None => "{closure}".to_string(),
    };
    let mut pre = cc.captured;
    if let Some(t) = cc.bound_this {
        pre.retain(|(k, _)| k != "this");
        pre.push(("this".to_string(), t));
    }
    // The frame is built inside `invoke`, so the site is handed over the same
    // way the late-static-binding class is.
    with_host(|h| h.closure_site_for_next_call(cc.site));
    // `zend_closure_get_closure`: the frame runs with the closure's own called
    // scope, whatever forwarding the call site would otherwise have done.
    with_host(|h| h.pending_lsb = cc.called_scope);
    invoke(
        &frame,
        Signature {
            params: &cc.params,
            ret: cc.ret.as_ref(),
            deprecated: None,
        },
        cc.chunk,
        pre,
        args,
        named,
        cc.is_generator,
    )
}

/// The `Error` PHP raises for a value used as a callable that is not one.
///
/// Three distinct messages, and the reference distinguishes them: an array is
/// judged on its LENGTH (a callable array is exactly `[target, method]`), an
/// object that is not invokable names its class, and every scalar names its
/// type. All are catchable `Error`s — they used to surface as the scaffold-level
/// `php: value is not callable`, which no `try` block could see.
fn not_callable(v: &Value) -> String {
    let msg = with_host(|h| {
        if h.is_array(v) {
            "Array callback must have exactly two elements".to_string()
        } else if h.is_object(v) {
            let class = h.object_class(v).unwrap_or_else(|| "stdClass".to_string());
            format!("Object of type {class} is not callable")
        } else {
            // NOT `type_name_for_error`: that spells a bool as the literal
            // `true`/`false` (which is what an argument TypeError wants). This
            // message names the TYPE, so a bool is `bool`.
            let t = match v {
                Value::Bool(_) => "bool".to_string(),
                other => h.type_name_for_error(other),
            };
            format!("Value of type {t} is not callable")
        }
    });
    crate::builtins::throws_bare("Error", msg)
}

/// A callable value that is not a closure, resolved to the method it names.
///
/// PHP accepts four such forms and this is the ONE place that decodes them, so
/// every entry point agrees: `$f(…)`, `usort`, `array_map`,
/// `preg_replace_callback` and `Closure::fromCallable` all route through
/// [`call_value`]. Splitting the knowledge — which is what happened before, with
/// only `call_user_func` able to read the array and `"C::m"` forms — meant
/// `usort($a, [$obj, "cmp"])` raised "Array callback must have exactly two
/// elements" on an array that HAD two elements, and `array_map` silently
/// returned its input unmapped.
///
/// Returns `None` when `callee` is a plain function-name string (the caller
/// dispatches it by name) or is not callable at all.
pub(crate) fn callable_method(callee: &Value) -> Option<(String, String, Option<Value>)> {
    // Array callable — exactly `[target, method]`. An object target is an
    // instance call, a class-name string a static one.
    if with_host(|h| h.is_array(callee)) {
        let pairs = with_host(|h| h.array_pairs(callee)).unwrap_or_default();
        if pairs.len() != 2 {
            return None;
        }
        let target = pairs[0].1.clone();
        let method = with_host(|h| h.to_str(&pairs[1].1));
        return match with_host(|h| h.object_class(&target)) {
            Some(class) => Some((class, method, Some(target))),
            None => Some((with_host(|h| h.to_str(&target)), method, None)),
        };
    }
    // A non-array object is callable exactly when its class defines `__invoke`.
    if with_host(|h| h.is_object(callee)) {
        let class = with_host(|h| h.object_class(callee))?;
        if with_host(|h| h.class_has_method(&class, "__invoke")) {
            return Some((class, "__invoke".to_string(), Some(callee.clone())));
        }
        return None;
    }
    // `"Class::method"` — a static call spelled as one string.
    if let Value::Str(s) = callee {
        if let Some((class, method)) = s.as_str().split_once("::") {
            return Some((class.to_string(), method.to_string(), None));
        }
    }
    None
}

/// Why `callee` cannot be called AT ALL, or `None` if the call gets to start.
///
/// This is [`call_value`]'s own decision tree, stopping where that one begins to
/// dispatch: a closure and anything [`callable_method`] resolves are let through
/// untouched, because whether the method they name exists is a question for the
/// method table, not for this. What is left is exactly `call_value`'s two
/// terminal arms — an unresolvable name string and [`not_callable`] — so this can
/// only ever refuse a call that was going to be refused a moment later.
///
/// It exists so the refusal can happen BEFORE the arguments are evaluated, which
/// is where the reference puts it: `$n = [1]; $n(f())` prints nothing before the
/// `Error`, because PHP never gets as far as `f()`.
pub(crate) fn callvalue_refusal(callee: &Value) -> Option<String> {
    if with_host(|h| h.closure_of(callee)).is_some() {
        return None;
    }
    // A callable naming a method answers to the method table, and it answers the
    // same way [`call_value`] will a moment later — so the refusal for
    // `$n = "Nope::m"` or `$n = [$obj, "nope"]` is available here, before the
    // arguments, which is where the reference raises it.
    if let Some((class, method, this)) = callable_method(callee) {
        return callable_method_refusal(&class, &method, &this);
    }
    match callee {
        Value::Str(s) => (!crate::stdlib::callable::function_resolves(s)).then(|| {
            crate::builtins::throws_bare("Error", format!("Call to undefined function {s}()"))
        }),
        other => Some(not_callable(other)),
    }
}

/// Why a method reached through a *value* — `$f = "C::m"`, `$f = [$obj, "m"]`,
/// the same shapes handed to `call_user_func` and `array_map` — cannot be
/// called, as a CATCHABLE `Error`, or `None` when the call gets to start.
///
/// `$o->m()` and `C::m()` are screened by their own opcodes, which throw a real
/// `Error` a `catch` block can see. The value form reached [`call_method`]
/// directly, whose miss is a bare `Err` — the scaffold surfaces that as
/// `php: Call to undefined method C::m()` on stderr and stops the program, so
/// `try { $f(1); } catch (Error $e)` never ran its handler. The three arms below
/// are the reference's, in its order: an undeclared class is reported as the
/// missing CLASS (`$n = "Nope::m"; $n(1)` says `Class "Nope" not found`, never
/// naming the method), and only then is the method table consulted.
fn callable_method_refusal(class: &str, method: &str, this: &Option<Value>) -> Option<String> {
    // Synthesized callees with no class table to consult, exactly as
    // `static_method_plan` lets them through: `Closure::bind`/`fromCallable`,
    // and the enum helpers. An engine object's methods (`Generator::next`) are
    // reached by `builtin_type_methods`, not by a `ClassDef`, so a class the
    // table does not know but that DOES answer this method is left alone.
    if class.eq_ignore_ascii_case("Closure") {
        return None;
    }
    if !with_host(|h| h.class_exists(class)) {
        if with_host(|h| h.class_has_method(class, method)) {
            return None;
        }
        return Some(crate::builtins::throws_bare(
            "Error",
            format!("Class \"{}\" not found", display_class(class)),
        ));
    }
    if with_host(|h| h.is_enum_class(class))
        && matches!(
            method.to_ascii_lowercase().as_str(),
            "cases" | "from" | "tryfrom"
        )
    {
        return None;
    }
    // `__call` answers an instance callable; `__callStatic` a `"C::m"` one. The
    // same test `static_method_plan` makes, so the two forms cannot disagree.
    let has_this = this
        .as_ref()
        .and_then(|t| with_host(|h| h.object_class(t)))
        .is_some_and(|c| with_host(|h| h.class_is_a_pub(&c, class)));
    match with_host(|h| h.method_dispatch(class, method, has_this)) {
        MethodDispatch::Direct | MethodDispatch::Magic => None,
        MethodDispatch::Denied(msg) => Some(crate::builtins::throws_bare("Error", msg)),
        MethodDispatch::Undefined => Some(crate::builtins::throws_bare(
            "Error",
            format!(
                "Call to undefined method {}::{method}()",
                display_class(class)
            ),
        )),
    }
}

/// Why `callee(...)` — PHP 8.1 first-class callable syntax — cannot build a
/// closure, as a catchable `Error`, or `None` when it can.
///
/// The reference settles the callee where the syntax is written: `$o->nope(...)`
/// raises `Call to undefined method C::nope()` on the spot, even though the
/// closure is never invoked. Every refusal is worded exactly as the CALL would
/// have been worded, which is why this delegates to `callvalue_refusal` rather
/// than inventing a second set of messages.
///
/// `instance` says the syntax was spelled with `->`. It cannot be recovered from
/// the value: both `$s->m(...)` and `$s::m(...)` on `$s = "C"` arrive as the
/// array `["C", "m"]`, and only the first is refused — a non-object receiver is
/// reported by its TYPE, as `$s->m()` itself would report it.
pub fn fcc_refusal(callable: &Value, instance: bool) -> Option<String> {
    if instance {
        let pairs = with_host(|h| h.array_pairs(callable)).unwrap_or_default();
        if let [(_, recv), (_, name)] = pairs.as_slice() {
            if !with_host(|h| h.is_object_value(recv)) {
                let method = with_host(|h| h.to_str(name));
                let ty = with_host(|h| crate::builtins::receiver_type_name(h, recv));
                return Some(crate::builtins::throws_bare(
                    "Error",
                    format!("Call to a member function {method}() on {ty}"),
                ));
            }
        }
    }
    callvalue_refusal(callable)
}

/// Invoke a callable *value*: a closure handle runs its captured-plus-bound body
/// in a fresh scope; an array / `"C::m"` / `__invoke` object resolves through
/// `callable_method`; a plain string is dispatched by name through
/// `call_function`. Used by `$f(...)` calls and callback builtins (`array_map`).
pub fn call_value(callee: Value, args: Vec<Value>) -> Result<Value, String> {
    if let Some(cc) = with_host(|h| h.closure_of(&callee)) {
        return invoke_closure(cc, args, Vec::new());
    }
    if let Some((class, method, this)) = callable_method(&callee) {
        if let Some(e) = callable_method_refusal(&class, &method, &this) {
            return Err(e);
        }
        return call_method(&class, &method, this, args);
    }
    match callee {
        Value::Str(s) => call_function_dispatched(&s, args, Dispatch::Indirect),
        other => Err(not_callable(&other)),
    }
}

/// `call_value` with PHP 8.0 named arguments (for `$closure(name: v)` / `$f(...)`).
pub fn call_value_named(
    callee: Value,
    args: Vec<Value>,
    named: Vec<(String, Value)>,
) -> Result<Value, String> {
    if let Some(cc) = with_host(|h| h.closure_of(&callee)) {
        return invoke_closure(cc, args, named);
    }
    if let Some((class, method, this)) = callable_method(&callee) {
        if let Some(e) = callable_method_refusal(&class, &method, &this) {
            return Err(e);
        }
        return call_method_named(&class, &method, this, args, named);
    }
    match callee {
        Value::Str(s) => call_function_named(&s, args, named),
        other => Err(not_callable(&other)),
    }
}

/// `Closure::bind($fn, $obj, $scope)` / `$fn->bindTo($obj, $scope)` — a copy of the
/// closure with `$this` rebound to `obj` and its private-access scope set. `scope`
/// may be a class-name string, an object (its class is used), or null/"static"
/// (keep the current scope). A null `obj` unbinds `$this` (a static closure).
///
/// Port of `do_closure_bind` and the non-fake-closure checks of
/// `zend_valid_closure_binding` (`Zend/zend_closures.c`). Every refusal is a
/// warning and a NULL result, never an exception.
pub fn closure_bind(closure: &Value, obj: Value, scope: Option<Value>) -> Result<Value, String> {
    let this = matches!(obj, Value::Obj(_)).then_some(obj.clone());
    let Some(mut scope_class) = with_host(|h| resolve_bind_scope(h, closure, scope)) else {
        return Ok(Value::Undef);
    };
    let refusal = with_host(|h| {
        if this.is_some() && h.closure_is_static(closure) {
            return Some(
                "Cannot bind an instance to a static closure, this will be an error in PHP 9"
                    .to_string(),
            );
        }
        let current = h.closure_scope(closure).map(|s| s.to_ascii_lowercase());
        let internal = scope_class.as_deref().filter(|s| {
            let lower = s.to_ascii_lowercase();
            Some(&lower) != current.as_ref() && h.is_internal_class(&lower)
        });
        internal.map(|s| {
            format!(
                "Cannot bind closure to scope of internal class {s}, this will be an error in PHP 9"
            )
        })
    });
    if let Some(msg) = refusal {
        with_host(|h| h.warn(msg));
        return Ok(Value::Undef);
    }
    // `zend_create_closure_ex`: an object bound without any scope gets the dummy
    // scope `Closure`, so `self::` names `Closure` and no private member opens.
    if scope_class.is_none() && this.is_some() {
        scope_class = Some("Closure".to_string());
    }
    match with_host(|h| h.rebind_closure(closure, this, scope_class)) {
        Some(v) => Ok(v),
        None => Err("Closure::bind expects a Closure".to_string()),
    }
}

/// Resolve the `$scope` argument of `bind`/`bindTo` the way `do_closure_bind`
/// does: an object names its class; the exact string `"static"` — also what an
/// omitted argument defaults to — keeps the closure's current scope; any other
/// string must name a class; and `null` is the unscoped closure. The outer
/// `None` is the refusal for a class that does not exist, after its warning.
fn resolve_bind_scope(
    h: &mut PhpHost,
    closure: &Value,
    scope: Option<Value>,
) -> Option<Option<String>> {
    let s = match scope {
        None => return Some(h.closure_scope(closure)),
        Some(Value::Undef) => return Some(None),
        Some(o @ Value::Obj(_)) if h.is_object(&o) => return Some(h.object_class(&o)),
        Some(other) => h.to_str(&other),
    };
    if s == "static" {
        return Some(h.closure_scope(closure));
    }
    if !h.class_exists(&s) {
        h.warn(format!("Class \"{s}\" not found"));
        return None;
    }
    let lower = s.to_ascii_lowercase();
    Some(Some(match h.classes.get(&lower) {
        Some(d) => d.name.clone(),
        None => builtin_type(&lower).map_or(s, |(n, ..)| n.to_string()),
    }))
}

/// `$fn->call($obj, ...$args)` — bind `$fn` to `$obj` with the scope set to
/// `$obj`'s class (so it can reach that class's private members), then call it.
pub fn closure_call(closure: &Value, obj: Value, args: Vec<Value>) -> Result<Value, String> {
    // `call` always scopes to the bound object's class — pass the object as the
    // scope so `resolve_bind_scope` derives its class.
    let bound = closure_bind(closure, obj.clone(), Some(obj))?;
    // A refused binding has already warned; `call` then answers null.
    if matches!(bound, Value::Undef) {
        return Ok(Value::Undef);
    }
    call_value(bound, args)
}

/// Dispatch a method call on a `Closure` object: `bindTo`, `call`, or `__invoke`.
pub fn call_closure_method(
    closure: &Value,
    method: &str,
    args: Vec<Value>,
) -> Result<Value, String> {
    let mut args = args;
    match method.to_ascii_lowercase().as_str() {
        "bindto" => {
            let obj = if args.is_empty() {
                Value::Undef
            } else {
                args.remove(0)
            };
            let scope = if args.is_empty() {
                None
            } else {
                Some(args.remove(0))
            };
            closure_bind(closure, obj, scope)
        }
        "call" => {
            let obj = if args.is_empty() {
                Value::Undef
            } else {
                args.remove(0)
            };
            closure_call(closure, obj, args)
        }
        "__invoke" => call_value(closure.clone(), args),
        other => Err(crate::builtins::throws_bare(
            "Error",
            format!("Call to undefined method Closure::{other}()"),
        )),
    }
}

/// The method names [`call_closure_method`] dispatches, lowercased.
///
/// Stated as data beside that `match` so [`engine_method_refusal`] — which has
/// to answer "does this name exist" BEFORE the arguments run, where the
/// dispatcher's own answer would come too late — cannot drift away from it.
const CLOSURE_METHODS: &[&str] = &["bindto", "call", "__invoke"];

/// [`CLOSURE_METHODS`] for [`call_generator_method`].
const GENERATOR_METHODS: &[&str] = &[
    "current",
    "key",
    "next",
    "valid",
    "rewind",
    "send",
    "throw",
    "getreturn",
];

/// Why `$closure->m(…)` or `$generator->m(…)` is refused, as a catchable
/// `Error`, or `None` when the call dispatches.
///
/// A `Closure` and a `Generator` are objects with no PHP class, so neither
/// `PhpHost::method_dispatch` nor the class method table can answer for them;
/// they are dispatched by name in [`call_closure_method`] and
/// [`call_generator_method`]. This is that same decision, available early enough
/// to stop `$c->nope(f())` before `f` runs — which is where the reference stops
/// it.
pub fn engine_method_refusal(recv: &Value, method: &str) -> Option<String> {
    let (class, methods) = if with_host(|h| h.is_closure(recv)) {
        ("Closure", CLOSURE_METHODS)
    } else if with_host(|h| h.is_generator_val(recv)) {
        ("Generator", GENERATOR_METHODS)
    } else {
        return None;
    };
    let lname = method.to_ascii_lowercase();
    if methods.contains(&lname.as_str()) {
        return None;
    }
    Some(crate::builtins::throws_bare(
        "Error",
        format!("Call to undefined method {class}::{method}()"),
    ))
}

// ── objects (new / method dispatch / constants) ─────────────────────────────

/// Record on a freshly allocated Throwable where it was raised.
///
/// PHP fixes `file`, `line` and the backtrace when the exception object is
/// *constructed*, not when it is thrown — `throw $e;` on a later line still
/// reports the `new` site — and that is also the only moment the frames are
/// still on the stack. `getFile`/`getLine`/`getTraceAsString` read the three
/// back, and the uncaught-exception fatal prints all of them.
fn seed_throwable(class: &str, obj: &Value) {
    with_host(|h| {
        let cl = class.to_ascii_lowercase();
        if !h.class_is_a(&cl, "exception") && !h.class_is_a(&cl, "error") {
            return;
        }
        // A `new` run by a PHP-written prelude method stands for a throw from
        // an internal one, which has no line of its own: the exception takes
        // the file and line of the first frame below that runs user code.
        let top = h.scopes.len() - 1;
        let mut i = top;
        while i > 0 && h.scopes[i].name.as_deref().is_some_and(is_prelude_frame) {
            i -= 1;
        }
        let (file, line) = if i == top {
            (h.current_file().to_string(), h.cur_frame_line() as i64)
        } else {
            (h.scope_file(i).to_string(), h.scopes[i].line as i64)
        };
        let trace = h.backtrace();
        h.prop_set(obj, "file", Value::str(file));
        h.prop_set(obj, "line", Value::int(line));
        h.prop_set(obj, "trace", Value::str(trace));
    });
}

/// Zero-based positions of the parameters an internal function declares
/// `#[\SensitiveParameter]`, which a backtrace must never print.
///
/// Only functions that can THROW need an entry, because an internal frame exists
/// only for the duration of [`throw_from_internal`]; a library function that
/// merely returns never reaches a trace. Verified against the reference by
/// throwing out of each and reading back `getTraceAsString`.
fn sensitive_params(func: &str) -> &'static [usize] {
    match func.to_ascii_lowercase().as_str() {
        "hash_hmac" | "hash_hmac_file" => &[2], // $key
        "hash_pbkdf2" => &[1],                  // $password
        _ => &[],
    }
}

/// Throw a PHP exception *from inside a library function*, with the internal
/// call as a real backtrace frame.
///
/// PHP renders such a throw with the library function itself at `#0`:
///
/// ```text
/// Fatal error: Uncaught ValueError: range(): Argument #3 ($step) … in file:1
/// Stack trace:
/// #0 file(1): range(9, 10, 2)
/// #1 {main}
/// ```
///
/// A library function has no PHP call frame — it is Rust — so one is pushed for
/// exactly as long as the exception object takes to construct, which is when PHP
/// snapshots `file`, `line` and the trace (`seed_throwable`). The frame carries
/// the arguments as its hidden `@args` so the trace prints them through the same
/// `trace_arg` rendering a user frame uses, and its line is the CALLER's, which
/// is the line PHP reports for a throw out of an internal function.
///
/// Returns `Ok(Undef)`: the exception is recorded as pending, and the `bubbled`
/// helper at every call site halts the chunk the moment it sees one, so the value
/// is never observed. Reporting it as `Err` instead would surface the scaffold's
/// terse `php: …` fatal and lose the catchability.
pub fn throw_from_internal(
    func: &str,
    args: &[Value],
    class: &str,
    message: &str,
) -> Result<Value, String> {
    throw_from_internal_args(func, args, class, vec![Value::str(message.to_string())])
}

/// Report an engine-level fatal raised inside a library function, with the trace
/// frame for the call the way the reference prints it. Returns the error string
/// the VM stops on; nothing is thrown, so no `catch` can see it — contrast
/// `throw_from_internal`, which builds a real Throwable.
pub fn fatal_from_internal(func: &str, args: &[Value], message: &str) -> String {
    let body = with_host(|h| {
        let line = h.cur_frame_line();
        h.scopes.push(Scope {
            name: Some(func.to_string()),
            line,
            internal: true,
            ..Scope::default()
        });
        let argsarr = h.new_array();
        for a in args {
            h.arr_push_auto(&argsarr, a.clone());
        }
        h.set_var("@args", argsarr);
        let trace = h.backtrace();
        h.scopes.pop();
        let body = format!(
            "{message} in {} on line {line}\nStack trace:\n{trace}",
            h.current_file()
        );
        h.fatal("Fatal error", &body);
        body
    });
    format!("Fatal error:  {body}")
}

/// [`throw_from_internal`] with the exception's full constructor argument list,
/// for the throws whose `getCode()` or `getPrevious()` is part of the contract.
pub fn throw_from_internal_args(
    func: &str,
    args: &[Value],
    class: &str,
    ctor_args: Vec<Value>,
) -> Result<Value, String> {
    let sensitive = sensitive_params(func);
    with_host(|h| {
        let line = h.cur_frame_line();
        h.scopes.push(Scope {
            name: Some(func.to_string()),
            line,
            internal: true,
            ..Scope::default()
        });
        let argsarr = h.new_array();
        for (i, a) in args.iter().enumerate() {
            if sensitive.contains(&i) {
                // A `#[\SensitiveParameter]` argument never appears in a trace:
                // the engine substitutes a `SensitiveParameterValue` wrapper so a
                // password or key cannot leak into an error log. The wrapper is
                // built as a real object of that class so `trace_arg` renders it
                // `Object(SensitiveParameterValue)` through the ordinary path.
                let obj = h.objs.len() as u32;
                h.objs.push(PhpObj::Object {
                    class: "SensitiveParameterValue".to_string(),
                    props: IndexMap::new(),
                });
                h.arr_push_auto(&argsarr, Value::Obj(obj));
            } else {
                h.arr_push_auto(&argsarr, a.clone());
            }
        }
        h.set_var("@args", argsarr);
    });
    // `new_object` runs PHP (the exception constructor), so it must not be called
    // inside the borrow above.
    let exc = new_object(class, ctor_args);
    with_host(|h| {
        h.scopes.pop();
    });
    set_pending_throw(exc?);
    Ok(Value::Undef)
}

/// `new Class(args)`: allocate an object seeded with its (inherited) property
/// defaults, then run its constructor. Property-default and constructor code run
/// on fresh VMs, so no host borrow is held across them.
pub fn new_object(class: &str, args: Vec<Value>) -> Result<Value, String> {
    let obj = alloc_object(class)?;
    construct_object(&obj, class, args, Vec::new())?;
    Ok(obj)
}

/// The first half of `new`: the instance, with its property defaults, before
/// a constructor argument is evaluated — `ZEND_NEW` allocates it there, which
/// is why `new A(new B)` makes `A` object #1 and `B` #2.
pub fn alloc_object(class: &str) -> Result<Value, String> {
    let cl = class.to_ascii_lowercase();
    // Both refusals are catchable `Error`s in the reference; a bare `Err` here
    // stopped the program with a scaffold message no `try` block could see.
    if let Some(e) = with_host(|h| h.class_instantiation_error(class)) {
        return Err(crate::builtins::throws_bare("Error", e));
    }
    let Some(defaults) = with_host(|h| h.class_prop_default_chunks(&cl)) else {
        return Err(crate::builtins::throws_bare(
            "Error",
            format!("Class \"{class}\" not found"),
        ));
    };
    // Evaluate each property default (a constant expression chunk).
    let mut props: IndexMap<String, Value> = IndexMap::new();
    for (name, (declaring, chunk, uninit)) in defaults {
        if uninit {
            continue;
        }
        let v = run_in_class_scope(&declaring, chunk)?;
        props.insert(name, v);
    }
    // Allocate the instance. The class name is stored with its original casing
    // for display (`get_class`, an uncaught-exception fatal); all lookups
    // (`resolve_method`, `catch_matches`, …) lowercase internally.
    let obj = with_host(|h| {
        h.objs.push(PhpObj::Object {
            class: class.to_string(),
            props,
        });
        Value::Obj((h.objs.len() - 1) as u32)
    });
    seed_throwable(class, &obj);
    Ok(obj)
}

/// The second half of `new`: run the constructor, if one exists anywhere in
/// the chain, over the allocated instance.
pub fn construct_object(
    obj: &Value,
    class: &str,
    args: Vec<Value>,
    named: Vec<(String, Value)>,
) -> Result<(), String> {
    let cl = class.to_ascii_lowercase();
    if with_host(|h| h.method_declared(&cl, "__construct")) {
        // `static::` inside the constructor is the instantiated class in its
        // declared spelling, not the lowercased lookup key.
        with_host(|h| h.lsb_set_for_next_call(class));
        if named.is_empty() {
            call_method(&cl, "__construct", Some(obj.clone()), args)?;
        } else {
            call_method_named(&cl, "__construct", Some(obj.clone()), args, named)?;
        }
    }
    Ok(())
}

/// `new Class(...)` with PHP 8.0 named constructor arguments.
pub fn new_object_named(
    class: &str,
    args: Vec<Value>,
    named: Vec<(String, Value)>,
) -> Result<Value, String> {
    let obj = alloc_object(class)?;
    construct_object(&obj, class, args, named)?;
    Ok(obj)
}

/// Invoke `Class::method` (resolved up the parent chain) with `this` bound to
/// `$this` when present. Reuses the shared `invoke` frame handling, so methods
/// honor default and variadic parameters.
pub fn call_method(
    class: &str,
    method: &str,
    this: Option<Value>,
    args: Vec<Value>,
) -> Result<Value, String> {
    let method_l = method.to_ascii_lowercase();
    // `Closure::bind($fn, $obj, $scope)` / `Closure::fromCallable($c)` — the only
    // static `Closure` methods, synthesized here (there is no `Closure` class).
    if class.eq_ignore_ascii_case("Closure") {
        return call_closure_static(&method_l, args);
    }
    // Enum static helpers `cases()`/`from()`/`tryFrom()` are synthesized, not
    // declared, so they are handled before the ordinary method resolution.
    if with_host(|h| h.is_enum_class(class)) {
        match method_l.as_str() {
            "cases" => return enum_cases_all(class),
            "from" => {
                return enum_from(
                    class,
                    args.into_iter().next().unwrap_or(Value::Undef),
                    false,
                )
            }
            "tryfrom" => {
                return enum_from(class, args.into_iter().next().unwrap_or(Value::Undef), true)
            }
            _ => {}
        }
    }
    let Some((def_class, def)) = with_host(|h| h.resolve_method(class, &method_l)) else {
        return call_magic_call(class, method, this, args);
    };
    let pre = match this {
        Some(t) => vec![("this".to_string(), t)],
        None => Vec::new(),
    };
    // `static::` inside the body resolves to the class the call NAMED, which is
    // not `def_class` — the method may have been inherited from an ancestor. A
    // forwarding call (`self::`, `parent::`, `static::`) has already claimed the
    // slot with the caller's own, and keeps it.
    with_host(|h| h.lsb_set_for_next_call(class));
    invoke_with_locals(
        &format!("{def_class}::{method}"),
        Signature {
            params: &def.params,
            ret: def.ret.as_ref(),
            deprecated: def.deprecated.as_ref(),
        },
        def.chunk,
        pre,
        args,
        Vec::new(),
        def.is_generator,
        &def.locals,
    )
}

/// The `__call` / `__callStatic` fallback: a call that resolved to no method (or
/// to one out of reach) is re-dispatched as `__call($name, $args)`, with the
/// argument list packed into a PHP array exactly as the reference does.
///
/// Every entry point for a method call funnels here, so a `call_user_func`, a
/// first-class callable and an ordinary `$o->m()` all reach the catch-all.
pub fn call_magic_call(
    class: &str,
    method: &str,
    this: Option<Value>,
    args: Vec<Value>,
) -> Result<Value, String> {
    let packed = with_host(|h| {
        let arr = h.new_array();
        for a in &args {
            let v = h.deref(a.clone());
            let v = h.copy_on_assign(v);
            h.arr_push_auto(&arr, v);
        }
        arr
    });
    call_magic_call_packed(class, method, this, packed)
}

/// [`call_magic_call`] for a call written with named arguments. PHP 8 hands
/// `__call` a single array whose positional arguments keep their integer keys and
/// whose named ones appear under their own parameter names.
pub fn call_magic_call_named(
    class: &str,
    method: &str,
    this: Option<Value>,
    args: Vec<Value>,
    named: Vec<(String, Value)>,
) -> Result<Value, String> {
    if named.is_empty() {
        return call_magic_call(class, method, this, args);
    }
    let packed = with_host(|h| {
        let arr = h.new_array();
        for a in &args {
            let v = h.deref(a.clone());
            let v = h.copy_on_assign(v);
            h.arr_push_auto(&arr, v);
        }
        for (k, v) in &named {
            let v = h.deref(v.clone());
            let v = h.copy_on_assign(v);
            h.arr_set_key(&arr, &Value::str(k.clone()), v);
        }
        arr
    });
    call_magic_call_packed(class, method, this, packed)
}

/// [`call_magic_call`] with the argument array already built — the named-argument
/// form needs to add string keys to it before the call.
fn call_magic_call_packed(
    class: &str,
    method: &str,
    this: Option<Value>,
    packed: Value,
) -> Result<Value, String> {
    let has_this = this.is_some();
    let Some(magic) = with_host(|h| h.magic_call_name(class, has_this)) else {
        return Err(format!(
            "Call to undefined method {}::{method}()",
            display_class(class)
        ));
    };
    let magic_args = vec![Value::str(method.to_string()), packed];
    let Some((def_class, def)) =
        with_host(|h| h.resolve_method(class, &magic.to_ascii_lowercase()))
    else {
        return Err(format!(
            "Call to undefined method {}::{method}()",
            display_class(class)
        ));
    };
    let pre = match this {
        Some(t) => vec![("this".to_string(), t)],
        None => Vec::new(),
    };
    with_host(|h| h.lsb_set_for_next_call(class));
    invoke(
        &format!("{def_class}::{magic}"),
        Signature {
            params: &def.params,
            ret: def.ret.as_ref(),
            deprecated: def.deprecated.as_ref(),
        },
        def.chunk,
        pre,
        magic_args,
        Vec::new(),
        def.is_generator,
    )
}

/// Static `Closure::` helpers: `bind($fn, $obj, $scope)` and
/// `fromCallable($callable)`.
fn call_closure_static(method_l: &str, mut args: Vec<Value>) -> Result<Value, String> {
    match method_l {
        "bind" => {
            let closure = if args.is_empty() {
                Value::Undef
            } else {
                args.remove(0)
            };
            let obj = if args.is_empty() {
                Value::Undef
            } else {
                args.remove(0)
            };
            let scope = if args.is_empty() {
                None
            } else {
                Some(args.remove(0))
            };
            closure_bind(&closure, obj, scope)
        }
        // `Closure::fromCallable($c)` — a real `Closure` over `$c`, which is
        // what makes `instanceof Closure` and `get_class()` answer the way the
        // reference does. It used to return its argument unchanged, so a string
        // callable stayed a string.
        "fromcallable" => {
            const FRAME: &str = "Closure::fromCallable";
            if args.len() != 1 {
                let msg = format!(
                    "Closure::fromCallable() expects exactly 1 argument, {} given",
                    args.len()
                );
                return throw_from_internal(FRAME, &args, "ArgumentCountError", &msg);
            }
            let callable = args.into_iter().next().unwrap_or(Value::Undef);
            // A `Closure` is already one; the reference hands the same object
            // back rather than wrapping it.
            if with_host(|h| h.is_closure(&callable)) {
                return Ok(callable);
            }
            // The refusal quotes the same reason a `callable` library parameter
            // does, under a heading of its own.
            if let Some(why) = crate::stdlib::callable::callable_reason(&callable) {
                let msg = format!("Failed to create closure from callable: {why}");
                return throw_from_internal(
                    FRAME,
                    std::slice::from_ref(&callable),
                    "TypeError",
                    &msg,
                );
            }
            Ok(with_host(|h| {
                h.make_closure(
                    crate::FROM_CALLABLE_FORWARDER,
                    vec![("c".to_string(), callable)],
                )
            }))
        }
        other => Err(format!("call to undefined method Closure::{other}()")),
    }
}

/// `call_method` with PHP 8.0 named arguments.
pub fn call_method_named(
    class: &str,
    method: &str,
    this: Option<Value>,
    args: Vec<Value>,
    named: Vec<(String, Value)>,
) -> Result<Value, String> {
    let method_l = method.to_ascii_lowercase();
    let Some((def_class, def)) = with_host(|h| h.resolve_method(class, &method_l)) else {
        return call_magic_call_named(class, method, this, args, named);
    };
    let pre = match this {
        Some(t) => vec![("this".to_string(), t)],
        None => Vec::new(),
    };
    // `static::` inside the body resolves to the class the call NAMED, which is
    // not `def_class` — the method may have been inherited from an ancestor. A
    // forwarding call (`self::`, `parent::`, `static::`) has already claimed the
    // slot with the caller's own, and keeps it.
    with_host(|h| h.lsb_set_for_next_call(class));
    invoke_with_locals(
        &format!("{def_class}::{method}"),
        Signature {
            params: &def.params,
            ret: def.ret.as_ref(),
            deprecated: def.deprecated.as_ref(),
        },
        def.chunk,
        pre,
        args,
        named,
        def.is_generator,
        &def.locals,
    )
}

/// Run `body` with `obj` marked as the object being cloned, so the readonly
/// check knows to allow the one write PHP reopens inside `__clone`. Restores
/// the previous marker even when the hook throws, because a `__clone` that
/// throws must not leave the next write unguarded.
fn in_clone_hook<T>(obj: &Value, body: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let Value::Obj(id) = obj else { return body() };
    let prev = with_host(|h| h.cloning.replace(*id));
    let r = body();
    with_host(|h| h.cloning = prev);
    r
}

/// `clone $o` — duplicate the object, then run `__clone()` on the copy.
///
/// The hook runs on the NEW object with no arguments, and it may write
/// properties the outside world can no longer touch (a `readonly` one included,
/// which PHP allows precisely here so a clone can carry a fresh identity).
pub fn clone_object(v: Value) -> Result<Value, String> {
    // An `enum` case is a SINGLETON: the reference refuses to duplicate one, by
    // the same message a generator gets, so `clone E::A` never produces a second
    // instance that `===` would tell apart from the case.
    if let Some(class) = with_host(|h| h.object_class(&v).filter(|c| h.is_enum_class(c))) {
        return Err(crate::builtins::throws_bare(
            "Error",
            format!(
                "Trying to clone an uncloneable object of class {}",
                display_class(&class)
            ),
        ));
    }
    let Some(copy) = with_host(|h| h.clone_obj(&v)) else {
        // Not clonable. A live generator holds a suspended stack that cannot be
        // duplicated and says so by name; everything else never was an object.
        if with_host(|h| h.gen_id(&v).is_some()) {
            return Err(crate::builtins::throws_bare(
                "Error",
                "Trying to clone an uncloneable object of class Generator",
            ));
        }
        // `type_name_for_error`, not `debug_type`: `clone true` is rejected with
        // `true given`, not `bool given`. See the note on that method.
        let t = with_host(|h| h.type_name_for_error(&v));
        return Err(crate::builtins::throws_bare(
            "TypeError",
            format!("clone(): Argument #1 ($object) must be of type object, {t} given"),
        ));
    };
    if let Some(class) = with_host(|h| h.object_class(&copy)) {
        if with_host(|h| h.method_declared(&class.to_ascii_lowercase(), "__clone")) {
            in_clone_hook(&copy, || {
                call_method(
                    &class.to_ascii_lowercase(),
                    "__clone",
                    Some(copy.clone()),
                    Vec::new(),
                )
            })?;
        }
    }
    Ok(copy)
}

/// Run a constant or property initializer in the scope of the class that
/// declares it — see [`PhpHost::init_class_scope`].
fn run_in_class_scope(declaring: &str, chunk: Chunk) -> Result<Value, String> {
    with_host(|h| h.init_class_scope.push(declaring.to_string()));
    let r = run_chunk_on(chunk);
    with_host(|h| h.init_class_scope.pop());
    r
}

/// `Class::CONST` — evaluate the (inherited) constant initializer. On an `enum`, a
/// name that is not a real constant is resolved as an enum case singleton.
pub fn class_const(class: &str, name: &str) -> Result<Value, String> {
    if let Some((declaring, chunk)) = with_host(|h| h.resolve_const_chunk(class, name)) {
        if let Some(msg) = with_host(|h| h.const_access_denied(class, name)) {
            return Err(crate::builtins::throws_bare("Error", msg));
        }
        return run_in_class_scope(&declaring, chunk);
    }
    if let Some(r) = enum_case(class, name) {
        return r;
    }
    // Both of these are catchable `Error`s in PHP, and which one it is depends
    // on whether the class exists at all — a distinction `$cls::K` makes very
    // visible, since the name only shows up at run time.
    let (declared, display) = with_host(|h| {
        (
            h.class_exists(class),
            h.class_display_name(&class.to_ascii_lowercase()),
        )
    });
    if !declared {
        return Err(crate::builtins::throws_bare(
            "Error",
            format!("Class \"{}\" not found", display_class(class)),
        ));
    }
    Err(crate::builtins::throws_bare(
        "Error",
        format!("Undefined constant {display}::{name}"),
    ))
}

/// Resolve `Enum::Case` to its singleton instance (built once, then cached so
/// `Enum::Case === Enum::Case` holds by object identity). `None` when `class` is
/// not an enum or has no case of that name — the caller then falls through to the
/// normal class-constant lookup / error.
pub fn enum_case(class: &str, case: &str) -> Option<Result<Value, String>> {
    let lower = class.to_ascii_lowercase();
    if !with_host(|h| h.is_enum_class(&lower)) {
        return None;
    }
    let chunk_opt = with_host(|h| h.enum_case_chunk(&lower, case))?;
    let key = format!("{lower}::{case}");
    if let Some(v) = with_host(|h| h.enum_case_cached(&key)) {
        return Some(Ok(v));
    }
    let value = match chunk_opt {
        Some(chunk) => match run_chunk_on(chunk) {
            Ok(v) => Some(v),
            Err(e) => return Some(Err(e)),
        },
        None => None,
    };
    let obj = with_host(|h| h.new_enum_object(class, case, value));
    with_host(|h| h.enum_case_store(&key, obj.clone()));
    Some(Ok(obj))
}

/// `Enum::cases()` — an array of every case singleton, in declaration order.
pub fn enum_cases_all(class: &str) -> Result<Value, String> {
    let names = with_host(|h| h.enum_case_names(class));
    let arr = with_host(|h| h.new_array());
    for name in names {
        match enum_case(class, &name) {
            Some(Ok(v)) => with_host(|h| h.arr_push_auto(&arr, v)),
            Some(Err(e)) => return Err(e),
            None => {}
        }
    }
    Ok(arr)
}

/// `Enum::from(value)` / `Enum::tryFrom(value)` — the case whose backing `value`
/// matches. `tryFrom` yields null on no match; `from` is a hard error (PHP throws
/// a catchable `ValueError`; the scaffold surfaces it as a fatal).
pub fn enum_from(class: &str, needle: Value, is_try: bool) -> Result<Value, String> {
    let names = with_host(|h| h.enum_case_names(class));
    let spelled = with_host(|h| h.declared_class_name(class));
    let fname = if is_try { "tryFrom" } else { "from" };
    // The BACKING TYPE, read off the first case. It decides three things the
    // needle alone cannot: which values are accepted at all, what they are
    // coerced to, and how the failure message renders them. A pure enum has no
    // backing value and no `from` at all.
    let first = match names.first() {
        Some(n) => match enum_case(class, n) {
            Some(Ok(v)) => v,
            Some(Err(e)) => return Err(e),
            None => Value::Undef,
        },
        None => Value::Undef,
    };
    let want_int = match with_host(|h| h.prop_get(&first, "value")) {
        Value::Int(_) => true,
        Value::Str(_) => false,
        _ => {
            return Err(crate::builtins::throws_bare(
                "Error",
                format!("Call to undefined method {spelled}::{fname}()"),
            ))
        }
    };
    // Weak-mode parameter coercion, which the reference performs BEFORE looking
    // for a matching case: `E::from(1.5)` on an int-backed enum finds the case
    // for `1` (after deprecating the narrowing), and `E::from("z")` on one is a
    // TypeError rather than a "not a valid backing value" ValueError.
    let coerced = match enum_backing_coerce(&needle, want_int, &spelled, fname) {
        Coerced::Value(v) => v,
        // The argument was refused: `throw_from_internal` has already recorded
        // the exception (it returns `Ok` with a PENDING throw, not `Err`), so
        // its own result is what has to travel back.
        Coerced::Refused(r) => return r,
    };
    let needle_s = with_host(|h| h.to_str(&coerced));
    for name in names {
        let case = match enum_case(class, &name) {
            Some(Ok(v)) => v,
            Some(Err(e)) => return Err(e),
            None => continue,
        };
        let val = with_host(|h| h.prop_get(&case, "value"));
        if with_host(|h| h.to_str(&val)) == needle_s {
            return Ok(case);
        }
    }
    if is_try {
        return Ok(Value::Undef);
    }
    // A catchable ValueError, not a scaffold fatal. The needle is rendered as
    // the BACKING type, which is what it was coerced to: an int bare, a string
    // quoted. Rendering the value as passed reported `E::from(9)` on a
    // string-backed enum as `9` where the reference says `"9"`.
    let shown = if want_int {
        needle_s.clone()
    } else {
        format!("\"{needle_s}\"")
    };
    // `from` is a real call in the reference's trace — `#0 file(1): E::from(99)`
    // — and both halves of the name are the DECLARED spelling however the call
    // was written, so `MYENUM::FROM(99)` still reports `MyEnum::from(99)`.
    throw_from_internal(
        &format!("{spelled}::from"),
        &[needle],
        "ValueError",
        &format!("{shown} is not a valid backing value for enum {spelled}"),
    )
}

/// Coerce `Enum::from`'s argument to the enum's backing type under PHP's
/// weak-mode rules for its `string|int $value` parameter.
///
/// The union prefers `int`, so a float, a bool and `null` all narrow to an int
/// first and only then become a string for a string-backed enum: `E::from(1.5)`
/// is the case for `"1"` there, not for `"1.5"`. Each narrowing carries the
/// reference's own diagnostic.
fn enum_backing_coerce(v: &Value, want_int: bool, spelled: &str, fname: &str) -> Coerced {
    // The reference's TypeError carries a FRAME for the call — `#0 file(1):
    // E::from('z')` — exactly as its ValueError does, so it goes through the
    // same internal-throw path rather than a bare throw.
    let type_error = |v: &Value| -> Coerced {
        let what = with_host(|h| h.type_name_for_error(v));
        let want = if want_int { "int" } else { "string|int" };
        let msg = format!(
            "{spelled}::{fname}(): Argument #1 ($value) must be of type {want}, {what} given"
        );
        Coerced::Refused(throw_from_internal(
            &format!("{spelled}::{fname}"),
            std::slice::from_ref(v),
            "TypeError",
            &msg,
        ))
    };
    let as_int = |n: i64| -> Value {
        if want_int {
            Value::int(n)
        } else {
            Value::str(n.to_string())
        }
    };
    match v {
        // An object or an array satisfies neither member of the union.
        Value::Obj(_) => type_error(v),
        Value::Undef => {
            with_host(|h| {
                h.deprecated(format!(
                    "{spelled}::{fname}(): Passing null to parameter #1 ($value) of \
                     type string|int is deprecated"
                ))
            });
            Coerced::Value(as_int(0))
        }
        Value::Bool(b) => Coerced::Value(as_int(i64::from(*b))),
        Value::Int(n) => Coerced::Value(as_int(*n)),
        Value::Float(f) => {
            if f.fract() != 0.0 {
                let shown = with_host(|h| h.to_str(v));
                with_host(|h| {
                    h.deprecated(format!(
                        "Implicit conversion from float {shown} to int loses precision"
                    ))
                });
            }
            Coerced::Value(as_int(*f as i64))
        }
        // A string satisfies the union as itself for a string-backed enum. For
        // an int-backed one it has to be numeric — and a numeric one narrows in
        // silence unless the narrowing loses a fraction.
        Value::Str(s) => {
            if !want_int {
                return Coerced::Value(v.clone());
            }
            if !is_numeric_string(s) {
                return type_error(v);
            }
            let f = with_host(|h| h.to_number(v)).to_float();
            if f.fract() != 0.0 {
                with_host(|h| {
                    h.deprecated(format!(
                        "Implicit conversion from float-string \"{s}\" to int loses precision"
                    ))
                });
            }
            Coerced::Value(Value::int(f as i64))
        }
        other => type_error(other),
    }
}

/// The outcome of [`enum_backing_coerce`]: the coerced value, or the refusal
/// that has already been thrown.
enum Coerced {
    Value(Value),
    Refused(Result<Value, String>),
}

/// The `Error` a `Class::$name` access raises when the calling scope cannot
/// reach the static property, as `zend_std_get_static_property_with_info`
/// (Zend/zend_object_handlers.c) reports it through `zend_bad_property_access`:
/// the visibility and the class the access NAMED, so `B::$q` for a `protected`
/// declared on parent `A` says `B::$q`, unlike the instance-property message.
fn static_prop_denied(class: &str, name: &str) -> Option<String> {
    with_host(|h| {
        let (declaring, vis) = h.resolve_prop_vis(class, name)?;
        if h.visibility_allows(vis, &declaring) {
            return None;
        }
        let vname = match vis {
            Visibility::Private => "private",
            Visibility::Protected => "protected",
            Visibility::Public => return None,
        };
        let shown = h.class_display_name(&class.to_ascii_lowercase());
        Some(format!("Cannot access {vname} property {shown}::${name}"))
    })
}

/// `Class::$prop` read. On first access the (constant) initializer runs once and
/// is stored in the per-class static cell; subsequent reads return the cell.
pub fn static_prop_get(class: &str, name: &str) -> Result<Value, String> {
    let Some((key, chunk)) = with_host(|h| h.resolve_static_key(class, name)) else {
        return Err(crate::builtins::throws_bare(
            "Error",
            format!("Access to undeclared static property {class}::${name}"),
        ));
    };
    if let Some(msg) = static_prop_denied(class, name) {
        return Err(crate::builtins::throws_bare("Error", msg));
    }
    if let Some(v) = with_host(|h| h.get_static_stored(&key)) {
        return Ok(v);
    }
    let declaring = key
        .split_once("::")
        .map_or(key.as_str(), |(c, _)| c)
        .to_string();
    let v = run_in_class_scope(&declaring, chunk)?;
    with_host(|h| h.set_static_stored(&key, v.clone()));
    Ok(v)
}

/// `Class::$prop` read in isset mode: an undeclared or unreachable static
/// property is null rather than an `Error`. An unknown class is still the
/// ordinary `Class "X" not found`.
pub fn static_prop_get_quiet(class: &str, name: &str) -> Result<Value, String> {
    let declared = with_host(|h| h.resolve_static_key(class, name).is_some());
    if !declared {
        if !with_host(|h| h.class_exists(class)) {
            return Err(crate::builtins::throws_bare(
                "Error",
                format!("Class \"{}\" not found", display_class(class)),
            ));
        }
        return Ok(Value::Undef);
    }
    if static_prop_denied(class, name).is_some() {
        return Ok(Value::Undef);
    }
    static_prop_get(class, name)
}

/// `Class::$prop = val` — write the per-class static cell, initializing lazily.
pub fn static_prop_set(class: &str, name: &str, val: Value) -> Result<Value, String> {
    let Some((key, _)) = with_host(|h| h.resolve_static_key(class, name)) else {
        return Err(crate::builtins::throws_bare(
            "Error",
            format!("Access to undeclared static property {class}::${name}"),
        ));
    };
    if let Some(msg) = static_prop_denied(class, name) {
        return Err(crate::builtins::throws_bare("Error", msg));
    }
    with_host(|h| h.set_static_stored(&key, val.clone()));
    Ok(val)
}

/// `Class::$prop` as an array to write through — `ops::SPROP_ENSURE_ARRAY`.
pub fn static_prop_ensure_array(class: &str, name: &str) -> Result<Value, String> {
    let cur = static_prop_get(class, name)?;
    if with_host(|h| h.is_array(&cur)) {
        return Ok(cur);
    }
    let arr = with_host(|h| h.new_array());
    static_prop_set(class, name, arr.clone())?;
    Ok(arr)
}

/// Whether `v` is something `...` may unpack: an array, a Generator, or an
/// object following the `Traversable` protocols — the same shapes
/// [`foreach_prep`] drives. A PLAIN object is NOT one: `foreach` walks its
/// public properties, but a spread rejects it, so the two part ways here.
pub fn is_unpackable(v: &Value) -> bool {
    if with_host(|h| h.is_array(v) || h.is_generator_val(v)) {
        return true;
    }
    let Some(class) = with_host(|h| h.object_class(v)) else {
        return false;
    };
    with_host(|h| {
        h.class_has_method(&class, "getIterator")
            || (h.class_has_method(&class, "valid") && h.class_has_method(&class, "current"))
    })
}

/// Normalize a `foreach` subject to an iterable array. Arrays pass through; an
/// object is iterated eagerly into a `(key, value)` array: `IteratorAggregate`
/// via `getIterator()`, the `Iterator` protocol (`rewind`/`valid`/`current`/
/// `key`/`next`), or — as a fallthrough — its public properties. Eager
/// materialization is fine for the finite iterators phplang supports; an infinite
/// iterator would not terminate (documented). A non-iterable yields an empty array.
///
/// Building an array MERGES repeated keys, which a Traversable is free to
/// produce (`yield from` restarts its inner generator's keys at 0). A caller
/// that must see every element — `iterator_to_array($it, false)`,
/// `iterator_count` — takes [`traversable_pairs`] instead.
/// Whether a `foreach` walks `v` one step at a time rather than as the array of
/// its elements: a generator, or an object implementing `Iterator` — whose
/// `rewind`/`valid`/`current`/`key`/`next` run interleaved with the body, as
/// `zend_fe_fetch_object_helper` drives them.
pub fn is_lazy_iter(v: &Value) -> bool {
    with_host(|h| {
        h.is_generator_val(v)
            || h.object_class(v)
                .is_some_and(|c| h.class_is_a_pub(&c, "Iterator"))
    })
}

/// [`ops::FOREACH_ITER`]: what a `foreach` over `v` walks. An
/// `IteratorAggregate` is replaced by the iterator its `getIterator()`
/// answers, through any number of aggregates, as `zend_user_it_get_new_iterator`
/// does — and an answer that is not `Traversable` is that function's
/// exception. Anything else is walked as it is.
pub fn foreach_iter(v: Value) -> Result<Value, String> {
    let mut v = v;
    loop {
        let Some(class) = with_host(|h| h.object_class(&v)) else {
            return Ok(v);
        };
        if !with_host(|h| h.class_is_a_pub(&class, "IteratorAggregate")) {
            return Ok(v);
        }
        let it = call_method(&class, "getIterator", Some(v.clone()), Vec::new())?;
        if with_host(|h| h.pending_throw.is_some()) {
            return Ok(Value::Undef);
        }
        let traversable = with_host(|h| {
            h.is_generator_val(&it)
                || h.object_class(&it)
                    .is_some_and(|c| h.class_is_a_pub(&c, "Traversable"))
        });
        if !traversable {
            let shown = with_host(|h| h.instance_class(&v)).unwrap_or(class);
            let exc = new_object(
                "Exception",
                vec![Value::str(format!(
                    "Objects returned by {shown}::getIterator() must be traversable or implement interface Iterator"
                ))],
            )?;
            set_pending_throw(exc);
            return Ok(Value::Undef);
        }
        v = it;
    }
}

/// One `Iterator` method of a `foreach` subject that is not a generator.
///
/// The reference walks most SPL iterators through a native `get_iterator`
/// rather than their methods, so a trace shows no frame for those steps —
/// until a subclass overrides one of the five methods, which switches the
/// class to `zend_user_it_get_iterator` (`zend_implement_iterator`). The
/// prelude methods standing in for such a native walk run in a transparent
/// frame.
pub fn iter_call(v: &Value, method: &str) -> Result<Value, String> {
    /// The classes `ext/spl` gives a `get_iterator` of their own.
    const NATIVE: &[&str] = &[
        "arrayiterator",
        "recursivearrayiterator",
        "recursiveiteratoriterator",
        "recursivetreeiterator",
        "spldoublylinkedlist",
        "splqueue",
        "splstack",
        "splheap",
        "splminheap",
        "splmaxheap",
        "splpriorityqueue",
    ];
    let class = with_host(|h| h.object_class(v)).ok_or("not an iterator")?;
    let native = with_host(|h| {
        ["rewind", "valid", "current", "key", "next"]
            .iter()
            .all(|m| {
                h.resolve_method(&class, m)
                    .is_some_and(|(owner, _)| NATIVE.contains(&owner.as_str()))
            })
    });
    with_host(|h| h.pending_transparent = native);
    let r = call_method(&class, method, Some(v.clone()), Vec::new());
    with_host(|h| h.pending_transparent = false);
    r
}

pub fn foreach_prep(v: Value) -> Result<Value, String> {
    if with_host(|h| h.is_array(&v)) {
        return Ok(v);
    }
    let arr = with_host(|h| h.new_array());
    let set = |key: Value, cur: Value| {
        with_host(|h| match key {
            Value::Undef => h.arr_push_auto(&arr, cur),
            k => h.arr_set_key(&arr, &k, cur),
        })
    };
    if let Some(inner) = traverse(&v, &mut |k, c| set(k, c))? {
        return foreach_prep(inner);
    }
    Ok(arr)
}

/// Every `(key, value)` a `foreach` over `v` would bind, in order and with
/// repeated keys kept. `Value::Undef` as a key means the source named none (an
/// `Iterator` without `key()`), which an array build turns into an append.
pub fn traversable_pairs(v: Value) -> Result<Vec<(Value, Value)>, String> {
    if with_host(|h| h.is_array(&v)) {
        return Ok(with_host(|h| h.array_pairs(&v).unwrap_or_default()));
    }
    let mut out = Vec::new();
    if let Some(inner) = traverse(&v, &mut |k, c| out.push((k, c)))? {
        return traversable_pairs(inner);
    }
    Ok(out)
}

/// Drive one non-array `foreach` subject, handing each `(key, value)` to `sink`.
///
/// `Ok(Some(inner))` means `v` is an `IteratorAggregate` and nothing was
/// emitted: the caller walks `inner`, which may itself be an array.
fn traverse(v: &Value, sink: &mut dyn FnMut(Value, Value)) -> Result<Option<Value>, String> {
    // A Generator is not a class instance — it has no entry in the class table —
    // so it is driven through its own protocol before the object paths below.
    // Materializing CONSUMES it, exactly as the reference's
    // `iterator_to_array($gen)` does.
    if with_host(|h| h.is_generator_val(v)) {
        gen_foreach_rewind(v)?;
        if with_host(|h| h.pending_throw.is_some()) {
            return Ok(None);
        }
        while gen_valid(v)? {
            let cur = gen_current(v)?;
            let key = gen_key(v)?;
            sink(key, cur);
            gen_next(v)?;
        }
        return Ok(None);
    }
    let Some(class) = with_host(|h| h.object_class(v)) else {
        return Ok(None);
    };
    // IteratorAggregate: `getIterator()` returns the real iterator (or a backing
    // array, for the SPL preludes) — the caller recurses on it.
    if with_host(|h| h.class_has_method(&class, "getIterator")) {
        return call_method(&class, "getIterator", Some(v.clone()), Vec::new()).map(Some);
    }
    // Iterator protocol.
    if with_host(|h| h.class_has_method(&class, "valid") && h.class_has_method(&class, "current")) {
        if with_host(|h| h.class_has_method(&class, "rewind")) {
            iter_call(v, "rewind")?;
        }
        let has_key = with_host(|h| h.class_has_method(&class, "key"));
        // Bound the walk so a broken `valid()` cannot hang the interpreter.
        for _ in 0..100_000_000u64 {
            let valid = iter_call(v, "valid")?;
            if !with_host(|h| h.is_truthy(&valid)) {
                break;
            }
            let cur = iter_call(v, "current")?;
            let key = if has_key {
                iter_call(v, "key")?
            } else {
                Value::Undef
            };
            sink(key, cur);
            iter_call(v, "next")?;
        }
        return Ok(None);
    }
    // Plain object: the properties the CALLING scope may see — public ones
    // from outside, private and protected ones too from inside the class —
    // which is the set `get_object_vars` answers with.
    for (k, val) in with_host(|h| h.object_props_visible(v)) {
        sink(Value::str(k), val);
    }
    Ok(None)
}

/// Whether the run already displayed a fatal in PHP's own shape, so the CLI
/// wrapper reports only the exit status instead of printing a second, terse copy.
pub fn fatal_reported() -> bool {
    with_host(|h| h.fatal_reported())
}

/// Record a pending `return` value for the enclosing function frame.
pub fn set_return(v: Value) {
    with_host(|h| h.signal = Some(Signal::Return(v)));
}

/// Record a `break`/`continue` control signal for the enclosing `try` body (the
/// orchestrator relays it to the loop the `try` sits inside).
pub fn set_break(level: u32) {
    with_host(|h| h.signal = Some(Signal::Break(level)));
}

pub fn set_continue(level: u32) {
    with_host(|h| h.signal = Some(Signal::Continue(level)));
}

/// The level of the `break`/`continue` that ended the most recent `try` body.
/// Read by the dispatch code the compiler emits after `RUN_TRY`, which needs to
/// know which enclosing loop the signal was aimed at.
pub fn last_break_level() -> u32 {
    with_host(|h| h.last_break_level)
}

/// Record a thrown exception object; the caller's dispatcher then unwinds.
pub fn set_pending_throw(v: Value) {
    with_host(|h| h.pending_throw = Some(v));
}

/// Record the status an `exit`/`die` ended the request with; every frame then
/// unwinds exactly as a throw does.
pub fn set_pending_exit(status: i32) {
    with_host(|h| h.pending_exit = Some(status));
}

/// The status `exit`/`die` ended the run with, if one ran. Read by the CLI
/// wrapper after the program returns, which is where the process status comes
/// from — `exit(3)` must leave the shell a 3, not a 0.
pub fn pending_exit() -> Option<i32> {
    with_host(|h| h.pending_exit)
}

/// Whether the run is unwinding: an exception is in flight, or an `exit`/`die`
/// has ended the request. Both stop every enclosing frame, so every call
/// dispatcher tests this rather than the throw alone — a dispatcher that only
/// looked for the throw would run the statement after an `exit` in a callee.
pub fn unwinding() -> bool {
    with_host(|h| h.pending_throw.is_some() || h.pending_exit.is_some())
}

/// The control status of running one `try`/`catch`/`finally` sub-body.
enum TryStatus {
    Normal,
    /// An `exit`/`die` ran inside the body. Unlike every other status this one
    /// is terminal: no `catch` may claim it and no `finally` runs after it,
    /// which is what the reference does — `try { exit(5); } finally { echo
    /// "fin"; }` prints nothing and exits 5.
    Exit,
    Return(Value),
    Throw(Value),
    Break,
    Continue,
    /// A non-catchable Rust-level fatal (e.g. an undefined function) — carried so
    /// `finally` still runs before it surfaces.
    Fatal(String),
}

/// Run one detached `try`/`catch`/`finally` body on the current scope, then read
/// back whichever control signal it raised (throw > return/break/continue >
/// normal). Signals are *taken* here so the next body runs clean — the value is
/// re-stashed only at the final propagation moment (see `run_try_orchestrator`).
fn run_body(chunk: Chunk) -> TryStatus {
    match run_chunk_on(chunk) {
        Err(e) => TryStatus::Fatal(e),
        Ok(_) => with_host(|h| {
            // The exit status is left in place rather than taken: it is not a
            // signal the orchestrator relays, it is the status the process ends
            // with, and the CLI wrapper reads it after the whole run.
            if h.pending_exit.is_some() {
                return TryStatus::Exit;
            }
            if let Some(exc) = h.pending_throw.take() {
                return TryStatus::Throw(exc);
            }
            match h.signal.take() {
                Some(Signal::Return(v)) => TryStatus::Return(v),
                Some(Signal::Break(n)) => {
                    h.last_break_level = n;
                    TryStatus::Break
                }
                Some(Signal::Continue(n)) => {
                    h.last_break_level = n;
                    TryStatus::Continue
                }
                None => TryStatus::Normal,
            }
        }),
    }
}

/// Orchestrate a `try`/`catch`/`finally` by id (baked into the `RUN_TRY` call).
/// Returns a status code the compiler branches on: `0` normal, `1` return,
/// `2` throw, `3` break, `4` continue. The propagated value is stashed into the
/// matching host signal just before returning, so the parent chunk consumes it
/// immediately. `finally` runs unconditionally and its own non-normal status
/// overrides whatever the `try`/`catch` produced. The whole thing lives on the
/// Rust stack, so nested `try`s recurse cleanly and no signal leaks across
/// unrelated constructs.
pub fn run_try_orchestrator(id: i64) -> Result<i64, String> {
    let Some(def) = with_host(|h| h.try_defs.get(id as usize).cloned()) else {
        return Err(format!("internal: no try-def #{id}"));
    };

    // An `@expr` that throws never reaches its `SUPPRESS_POP`, so the region it
    // opened would stay open and silence everything after the catch. Unwinding
    // out of the expression restores the depth, which is what the reference does
    // when it restores the error-reporting level on the way out.
    let suppress_depth = with_host(|h| h.suppress_depth());
    let mut status = run_body(def.try_chunk);
    with_host(|h| h.suppress_restore(suppress_depth));

    // An `exit` in the body ends the request there: no `catch` is consulted and
    // no `finally` runs. Reported as the same status code a throw uses, so the
    // parent chunk halts — but with nothing stashed for a `catch` to find.
    if matches!(status, TryStatus::Exit) {
        return Ok(2);
    }

    // A thrown exception: try each catch clause in order; the first whose union
    // of class names matches the thrown object's class wins.
    if let TryStatus::Throw(exc) = &status {
        if let Some(class) = with_host(|h| h.object_class(exc)) {
            let exc = exc.clone();
            for c in &def.catches {
                let matched = with_host(|h| c.classes.iter().any(|t| h.catch_matches(&class, t)));
                if matched {
                    if let Some(var) = &c.var {
                        with_host(|h| h.set_var(var, exc.clone()));
                    }
                    status = run_body(c.chunk.clone());
                    break;
                }
            }
        }
    }

    // `finally` always runs; a non-normal status from it replaces the pending one.
    if let Some(fin) = def.finally_chunk {
        match run_body(fin) {
            TryStatus::Normal => {}
            other => status = other,
        }
    }

    Ok(match status {
        TryStatus::Normal => 0,
        // An `exit` in a `catch` or `finally` body — same terminal handling as
        // one in the `try` body above, reached here because those two run after
        // the early return.
        TryStatus::Exit => 2,
        TryStatus::Return(v) => {
            with_host(|h| h.signal = Some(Signal::Return(v)));
            1
        }
        TryStatus::Throw(v) => {
            with_host(|h| h.pending_throw = Some(v));
            2
        }
        TryStatus::Break => 3,
        TryStatus::Continue => 4,
        TryStatus::Fatal(e) => return Err(e),
    })
}

// ── numeric formatting / parsing helpers ──────────────────────────────────

/// Format a float the way PHP's default `precision=14` echo does.
fn float_to_php_string(f: f64) -> String {
    if f.is_nan() {
        return "NAN".into();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-INF" } else { "INF" }.into();
    }
    if f == 0.0 {
        // PHP prints negative zero as "-0".
        return if f.is_sign_negative() { "-0" } else { "0" }.into();
    }
    // PHP's default echo uses `zend_gcvt` with precision 14 (significant digits):
    // fixed notation in the normal range, scientific (`1.5E-10`, `1.0E+100`) once
    // the decimal exponent is < -4 or >= 15.
    php_gcvt(f, 14)
}

/// Format a positive-or-negative float the way PHP renders it — 14 significant
/// digits, `%G`-style choice of fixed vs scientific. Also used by `sprintf`'s
/// `%g` with a caller-supplied precision.
pub fn php_gcvt(f: f64, precision: usize) -> String {
    if f == 0.0 {
        return if f.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    let neg = f < 0.0;
    let a = f.abs();
    let exp = a.log10().floor() as i32;
    // `%G` rule: scientific when the exponent is below -4 or reaches the
    // significant-digit precision (PHP's default precision is 14).
    let sci = exp < -4 || exp >= precision as i32;
    let body = if sci {
        // Mantissa with (precision-1) fractional digits, trailing zeros stripped
        // but a decimal point kept (`1.0E+100`); explicit exponent sign.
        let raw = format!("{:.*e}", precision.saturating_sub(1), a);
        let (mant, ex) = raw.split_once('e').unwrap_or((raw.as_str(), "0"));
        let mant = mant.trim_end_matches('0');
        // Keep exactly one fractional digit when the mantissa is integer-valued:
        // "1." → "1.0", "1" → "1.0"; leave "1.844674407371" untouched.
        let mant = if let Some(stripped) = mant.strip_suffix('.') {
            format!("{stripped}.0")
        } else if mant.contains('.') {
            mant.to_string()
        } else {
            format!("{mant}.0")
        };
        let exp_n: i32 = ex.parse().unwrap_or(0);
        format!(
            "{mant}E{}{}",
            if exp_n < 0 { "-" } else { "+" },
            exp_n.abs()
        )
    } else {
        let decimals = (precision as i32 - 1 - exp).max(0) as usize;
        let s = format!("{a:.decimals$}");
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        }
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

/// The whitespace PHP's numeric-string scanner skips. Deliberately not
/// `str::trim`, which also strips Unicode spaces: PHP reads `"\u{a0}5"` as
/// non-numeric, and trimming NBSP here would silently make it `5`.
const PHP_SPACE: &[char] = &[' ', '\t', '\n', '\r', '\x0b', '\x0c'];

/// Scan the leading numeric run of `s`, returning its value and how many bytes
/// of `s.trim_start_matches(PHP_SPACE)` it consumed, or `None` when the string
/// does not begin with a number at all.
///
/// This is the single source of truth behind all three questions PHP asks about
/// a string in a numeric context — is it numeric, does it merely *start*
/// numeric, and what is the number — so the three can never disagree.
///
/// Two rules are easy to get wrong and are what the byte scan is here for:
/// an exponent counts only when digits actually follow it, so `"5e"` reads as
/// `5` rather than failing; and `"5."` and `".5"` are both complete numbers
/// while a bare `"."` is not.
fn scan_php_number(s: &str) -> Option<(Value, usize, bool)> {
    let t = s.trim_start_matches(PHP_SPACE);
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let digits_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    // `end` is the length of the longest complete number seen so far; 0 means
    // nothing valid has been scanned yet, which is how a bare sign or `.` fails.
    let mut end = if i > digits_start { i } else { 0 };
    let mut is_float = false;
    if i < b.len() && b[i] == b'.' {
        let frac_start = i + 1;
        let mut j = frac_start;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        // `".5"` needs the fraction digits; `"5."` is already complete without
        // them. A lone `"."` has neither and stays invalid.
        if j > frac_start || end != 0 {
            i = j;
            end = j;
            is_float = true;
        }
    }
    if end == 0 {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exp_digits = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_digits {
            end = j;
            is_float = true;
        }
    }
    let text = &t[..end];
    // An integer literal too wide for `i64` becomes a float, as PHP does.
    let value = match (is_float, text.parse::<i64>()) {
        (false, Ok(n)) => Value::int(n),
        _ => Value::float(text.parse::<f64>().ok()?),
    };
    Some((value, end, is_float))
}

/// Whether a string's numeric prefix is *written* in float form — it carries a
/// decimal point or an exponent.
///
/// This is not the same question as "did it parse to a float": an integer-format
/// string too wide for `i64` also parses to a float, and PHP treats the two
/// differently when narrowing back to int. `"1e20" % 2` deprecates the
/// narrowing, `"9223372036854775808" % 2` does not.
pub fn numeric_prefix_is_float(s: &str) -> bool {
    scan_php_number(s).is_some_and(|(_, _, is_float)| is_float)
}

/// Parse the leading numeric prefix of a string into an `Int`/`Float` `Value`,
/// as PHP does when a string is used in arithmetic (`"12abc" + 0 == 12`).
/// A string with no numeric prefix at all reads as `0`, matching `(int)"abc"`.
fn parse_php_number(s: &str) -> Value {
    scan_php_number(s).map_or(Value::int(0), |(v, _, _)| v)
}

/// Whether a string is a fully numeric PHP string (for loose comparison).
/// PHP's alphanumeric string succession, the `++` operator on a non-numeric
/// string (`"a"` → `"b"`, `"z"` → `"aa"`, `"Az"` → `"Ba"`, `"a9"` → `"b0"`).
///
/// Carry propagates right to left through `[a-zA-Z0-9]` only: the first
/// non-alphanumeric character stops it outright, leaving the rest of the string
/// untouched (`"a-z"` → `"a-a"`, `"a_"` unchanged). A carry that runs off the
/// front prepends a character of the same class as the one it left — `a`, `A` or
/// `1`.
fn increment_alnum_string(s: &str) -> String {
    let mut b = s.as_bytes().to_vec();
    let mut i = b.len();
    let mut carry = false;
    while i > 0 {
        i -= 1;
        match b[i] {
            b'z' => {
                b[i] = b'a';
                carry = true;
            }
            b'Z' => {
                b[i] = b'A';
                carry = true;
            }
            b'9' => {
                b[i] = b'0';
                carry = true;
            }
            c if c.is_ascii_alphanumeric() => {
                b[i] = c + 1;
                carry = false;
                break;
            }
            // A non-alphanumeric character ends the succession entirely.
            _ => {
                carry = false;
                break;
            }
        }
        if !carry {
            break;
        }
    }
    if carry {
        let lead = match b.first() {
            Some(b'0') => b'1',
            Some(b'A') => b'A',
            _ => b'a',
        };
        b.insert(0, lead);
    }
    String::from_utf8_lossy(&b).into_owned()
}

pub fn is_numeric_string(s: &str) -> bool {
    parse_php_number_full(s).is_some()
}

/// The int key a STRING array key folds to, or `None` when it stays a string.
///
/// PHP folds a key that is written as a CANONICAL decimal integer — no sign
/// but `-`, no leading zeros, no `+`, and inside `i64` — and leaves every other
/// string alone. The shape is checked BEFORE parsing so the common non-numeric
/// key (`"name"`, `"k12"`) is rejected on its first byte, and the old
/// round-trip test `n.to_string() == **s`, which allocated a `String` for every
/// numeric-looking key just to compare it, is gone.
pub fn canonical_int_key(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let digits = match b.first()? {
        b'-' => &b[1..],
        _ => b,
    };
    // No empty digit run, no leading zero (but "0" itself folds), and "-0" is
    // not canonical — PHP keeps it as the string key it was written as.
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if digits[0] == b'0' && (digits.len() > 1 || b[0] == b'-') {
        return None;
    }
    s.parse::<i64>().ok()
}

/// How PHP reads the subscript of a STRING receiver, `$s[k]`.
pub enum StrOffset {
    /// A byte offset. The flag is set when the key had to be CONVERTED to get
    /// it, which the reference reports as `String offset cast occurred` — a
    /// bool, a null, or ANY float, `$s[1.0]` included.
    At(i64, bool),
    /// No offset exists. A value-context read or a write throws `Cannot access
    /// offset of type <this> on string`; `isset()` and `??` answer quietly.
    Bad,
}

/// Classify a string subscript. See [`StrOffset`].
///
/// An integer key is used as-is. A STRING key is accepted only when it is
/// written as a plain integer — surrounding whitespace, a sign, and leading
/// zeros are all fine (`" 1"`, `"-1"`, `"01"`) — because that is what
/// `ZEND_HANDLE_NUMERIC_STRING` accepts. `"1.5"`, `"1e0"`, and `"x"` are not
/// offsets at all, and used to be read through `Value::to_int`, which answers
/// 0 for each of them and silently returned the FIRST byte.
pub fn classify_string_offset(key: &Value) -> StrOffset {
    match key {
        Value::Int(n) => StrOffset::At(*n, false),
        Value::Bool(b) => StrOffset::At(i64::from(*b), true),
        Value::Undef => StrOffset::At(0, true),
        Value::Float(f) => StrOffset::At(dval_to_lval(*f), true),
        Value::Str(s) => {
            let t = s.trim_matches(PHP_SPACE);
            let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
            match (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
                .then(|| t.parse::<i64>().ok())
                .flatten()
            {
                Some(n) => StrOffset::At(n, false),
                None => StrOffset::Bad,
            }
        }
        _ => StrOffset::Bad,
    }
}

/// `zend_dval_to_lval`: narrow a double to `int` the way PHP does.
///
/// Rust's `as i64` SATURATES, so `(int) 1e19` answered `PHP_INT_MAX`. PHP wraps
/// modulo 2^64 (`zend_dval_to_lval_slow`) and answers -8446744073709551616.
/// `fmod` is exact on IEEE doubles, so the wrap loses nothing the input did not
/// already lack. A non-finite double is 0.
pub fn dval_to_lval(f: f64) -> i64 {
    const TWO_63: f64 = 9223372036854775808.0; // 2^63
    const TWO_64: f64 = 18446744073709551616.0; // 2^64
    if !f.is_finite() {
        return 0;
    }
    if (-TWO_63..TWO_63).contains(&f) {
        return f as i64;
    }
    let wrapped = f.trunc().rem_euclid(TWO_64);
    if wrapped >= TWO_63 {
        (wrapped - TWO_64) as i64
    } else {
        wrapped as i64
    }
}

/// Parse a string that is *entirely* numeric (no trailing garbage), or `None`.
///
/// "Entirely" allows whitespace on both ends (`" 5 "` is numeric in PHP 8) but
/// nothing else. Routing through `scan_php_number` rather than Rust's own
/// float parser is what keeps `"INF"`/`"NAN"` non-numeric — Rust accepts those
/// spellings and PHP does not — while still admitting `"1e400"`, which is a
/// numeric string that happens to overflow to infinity.
pub fn parse_php_number_full(s: &str) -> Option<Value> {
    let t = s.trim_matches(PHP_SPACE);
    let (value, used, _) = scan_php_number(t)?;
    (used == t.len()).then_some(value)
}

/// How an operand reads to PHP 8's arithmetic operators.
///
/// PHP 8 split what PHP 7 did silently into three outcomes, and an operator has
/// to tell them apart *before* it computes anything. See [`classify_arith`].
pub enum ArithOperand {
    /// Converts silently: a number, a bool, null, or a fully numeric string.
    Numeric(Value),
    /// A leading-numeric string such as `"5g"`. The operator raises
    /// `A non-numeric value encountered` and then uses the prefix.
    Leading(Value),
    /// No numeric reading at all — `"g"`, `""`, an array, an object. The
    /// operator raises `TypeError` instead of producing a value.
    Unsupported,
}

/// Classify an operand for PHP 8's arithmetic operators.
///
/// The three-way split is the whole of the PHP 7 → 8 juggling change: `"g" + 9`
/// used to be `9` and is now a `TypeError`, while `"5g" + 1` used to be silent
/// and is now a warning that still yields `6`. Booleans and `null` were left
/// alone by that change and still convert without complaint.
/// Takes no host: every arm is decided by the operand ALONE. It used to take
/// one and ignore it, which cost the caller a thread-local lookup and a
/// `RefCell` borrow of the whole host per arithmetic operand — on the hot path
/// of every `%`, `intdiv`, and bitwise operator in a program.
pub fn classify_arith(v: &Value) -> ArithOperand {
    match v {
        Value::Int(_) | Value::Float(_) => ArithOperand::Numeric(v.clone()),
        Value::Bool(b) => ArithOperand::Numeric(Value::int(*b as i64)),
        Value::Undef => ArithOperand::Numeric(Value::int(0)),
        Value::Str(s) => match parse_php_number_full(s) {
            Some(n) => ArithOperand::Numeric(n),
            // A prefix exists but did not cover the string: warn and use it.
            None => match scan_php_number(s) {
                Some((n, _, _)) => ArithOperand::Leading(n),
                None => ArithOperand::Unsupported,
            },
        },
        // Arrays and objects have no arithmetic reading. `__toString` is not
        // consulted here: the reference reports `Closure + int`, not `string`.
        _ => ArithOperand::Unsupported,
    }
}

/// One of PHP's four type declaration kinds. `class_exists`, `interface_exists`,
/// `trait_exists` and `enum_exists` each answer for exactly one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    Class,
    Interface,
    Trait,
    Enum,
}

/// The engine types PHP declares that this runtime implements without a
/// `ClassDef` — the two object kinds that are `PhpObj` variants rather than
/// class instances, and the interfaces the engine defines for user classes to
/// implement. Each entry is `(name, kind, transitive parents, method names) —
/// every name in PHP's own spelling, matched case-insensitively, because
/// `class_implements` reports the spelling and not the lookup key.
///
/// The parents are what `instanceof`/`is_a` walk when the name has no entry in
/// the class table: `$gen instanceof Traversable` is true, and nothing in the
/// class table says so.
///
/// Deliberately NOT here: engine classes this runtime does not implement
/// (`WeakMap`, `Attribute`, `Fiber`). Answering `class_exists` yes for a name
/// that `new` cannot build trades one wrong answer for a worse one.
type BuiltinType = (
    &'static str,
    TypeKind,
    &'static [&'static str],
    &'static [&'static str],
);

const BUILTIN_TYPES: &[BuiltinType] = &[
    (
        "Closure",
        TypeKind::Class,
        &[],
        &["bind", "bindTo", "call", "fromCallable", "getCurrent"],
    ),
    (
        "Generator",
        TypeKind::Class,
        &["Iterator", "Traversable"],
        &[
            "rewind",
            "valid",
            "current",
            "key",
            "next",
            "send",
            "throw",
            "getReturn",
            "__debugInfo",
        ],
    ),
    ("Traversable", TypeKind::Interface, &[], &[]),
    (
        "Iterator",
        TypeKind::Interface,
        &["Traversable"],
        &["current", "next", "key", "valid", "rewind"],
    ),
    (
        "IteratorAggregate",
        TypeKind::Interface,
        &["Traversable"],
        &["getIterator"],
    ),
    ("Countable", TypeKind::Interface, &[], &["count"]),
    (
        "ArrayAccess",
        TypeKind::Interface,
        &[],
        &["offsetExists", "offsetGet", "offsetSet", "offsetUnset"],
    ),
    ("Stringable", TypeKind::Interface, &[], &["__toString"]),
    (
        "JsonSerializable",
        TypeKind::Interface,
        &[],
        &["jsonSerialize"],
    ),
    (
        "Throwable",
        TypeKind::Interface,
        &["Stringable"],
        &[
            "getMessage",
            "getCode",
            "getFile",
            "getLine",
            "getTrace",
            "getPrevious",
            "getTraceAsString",
            "__toString",
        ],
    ),
    ("UnitEnum", TypeKind::Interface, &[], &["cases"]),
    (
        "BackedEnum",
        TypeKind::Interface,
        &["UnitEnum"],
        &["from", "tryFrom", "cases"],
    ),
];

/// The entry for a built-in type name, matched case-insensitively.
fn builtin_type(name: &str) -> Option<&'static BuiltinType> {
    BUILTIN_TYPES
        .iter()
        .find(|(n, ..)| n.eq_ignore_ascii_case(name))
}

/// The kind of a built-in type name, or `None`.
fn builtin_type_kind(name: &str) -> Option<TypeKind> {
    builtin_type(name).map(|(_, k, ..)| *k)
}

/// The transitive ancestors of a built-in type name, in PHP's spelling.
fn builtin_type_parents(name: &str) -> &'static [&'static str] {
    builtin_type(name).map_or(&[][..], |(_, _, p, _)| *p)
}

/// The methods a built-in type declares, in PHP's spelling.
fn builtin_type_methods(name: &str) -> &'static [&'static str] {
    builtin_type(name).map_or(&[][..], |(.., m)| *m)
}

/// A class name as a *message* shows it: everything up to the first NUL.
///
/// Only an anonymous class has one — its name is
/// `Base@anonymous\0<script>:<line>$<n>`, a single string carrying both a
/// readable head and the suffix that makes it unique. The reference passes that
/// name to its diagnostics and dumpers as a C string, so they print the head
/// alone, while `get_class`/`::class`/`var_export` hand back the whole thing.
/// A name with no NUL — every other class — is returned unchanged.
pub fn display_class(name: &str) -> &str {
    match name.split_once('\0') {
        Some((head, _)) => head,
        None => name,
    }
}

/// The first `final` rule the class `def` breaks against what it inherits, as
/// the reference reports it when it links the class: the message, plus the
/// line to report it at when that is not the declaration's own (an override
/// of a final method is reported at the overriding method). `find` resolves a
/// class or interface name, either case, to its definition.
///
/// The reference's order is kept: a final parent class first, then the
/// properties, the constants and the methods, each in the order the ancestor
/// declared them. Only the NEAREST ancestor declaring a member decides
/// whether it is final, which is also the class the message names.
pub fn final_violation<'a>(
    def: &ClassDef,
    find: impl Fn(&str) -> Option<&'a ClassDef>,
) -> Option<(String, Option<u32>)> {
    // The parent chain, nearest first. A cycle cannot link, but is not this
    // check's to report, so it only stops the walk.
    let mut chain: Vec<&ClassDef> = Vec::new();
    let mut next = def.parent.as_deref();
    while let Some(p) = next {
        let Some(d) = find(p) else { break };
        if chain.iter().any(|c| std::ptr::eq(*c, d)) {
            break;
        }
        chain.push(d);
        next = d.parent.as_deref();
    }
    if let Some(parent) = chain.first() {
        if parent.is_final {
            return Some((
                format!(
                    "Class {} cannot extend final class {}",
                    display_class(&def.name),
                    parent.name
                ),
                None,
            ));
        }
    }

    let mut seen: FxHashSet<&str> = FxHashSet::default();
    for anc in &chain {
        let names = anc.prop_defaults.iter().chain(&anc.static_prop_defaults);
        for (name, _) in names {
            if seen.insert(name)
                && anc.final_props.contains(name)
                && def.prop_vis.contains_key(name)
            {
                return Some((
                    format!("Cannot override final property {}::${name}", anc.name),
                    None,
                ));
            }
        }
    }

    // Constants come from interfaces as well: every interface the class or an
    // ancestor implements, and every interface those extend.
    let mut ifaces: Vec<&ClassDef> = Vec::new();
    let mut pending: Vec<&str> = def
        .interfaces
        .iter()
        .chain(chain.iter().flat_map(|c| &c.interfaces))
        .map(String::as_str)
        .collect();
    let mut i = 0;
    while i < pending.len() {
        if let Some(d) = find(pending[i]) {
            if !ifaces.iter().any(|x| std::ptr::eq(*x, d)) {
                ifaces.push(d);
                pending.extend(d.interfaces.iter().map(String::as_str));
            }
        }
        i += 1;
    }
    seen.clear();
    for anc in chain.iter().chain(&ifaces) {
        for (name, _) in &anc.consts {
            if seen.insert(name)
                && anc.final_consts.contains(name)
                && def.consts.iter().any(|(n, _)| n == name)
            {
                return Some((
                    format!(
                        "{}::{name} cannot override final constant {}::{name}",
                        display_class(&def.name),
                        anc.name
                    ),
                    None,
                ));
            }
        }
    }

    let mut seen: FxHashSet<String> = FxHashSet::default();
    for anc in &chain {
        for (spelled, _) in &anc.method_sites {
            let key = spelled.to_ascii_lowercase();
            if !seen.insert(key.clone()) || !anc.final_methods.contains(&key) {
                continue;
            }
            let own = def
                .method_sites
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(spelled));
            if let Some((own, line)) = own {
                return Some((
                    format!("Cannot override final method {}::{own}()", anc.name),
                    Some(*line),
                ));
            }
        }
    }
    None
}

/// The type name PHP prints in `Unsupported operand types: X op Y`.
///
/// These are the short spellings (`int`, `bool`, `null`), not the `gettype`
/// ones ([`PhpHost::type_name`] returns `integer`/`boolean`/`NULL`), and an
/// object contributes its class name rather than the word `object`.
pub fn arith_type_name(h: &PhpHost, v: &Value) -> String {
    match v {
        Value::Undef => "null".into(),
        Value::Bool(_) => "bool".into(),
        Value::Int(_) => "int".into(),
        Value::Float(_) => "float".into(),
        Value::Str(_) => "string".into(),
        Value::Obj(_) if h.is_array(v) => "array".into(),
        Value::Obj(_) if h.is_stream(v) => "resource".into(),
        Value::Obj(_) => h.instance_class(v).unwrap_or_else(|| "object".into()),
        _ => "mixed".into(),
    }
}

// ── include / require / eval ────────────────────────────────────────────────

/// A parameter as a call site needs it: `(name, by_ref, variadic)`.
pub type ParamShape = (String, bool, bool);

/// The `include_path` a failed `include` quotes. phplang searches the working
/// directory and then the including file's directory, and has no PEAR
/// directory to add.
pub const INCLUDE_PATH: &str = ".";

impl PhpHost {
    /// `(lowercased name, [(parameter, by_ref, variadic)])` for every loaded user
    /// function — what a run-time compilation needs to write a by-reference
    /// argument back at a call to a function it did not see declared.
    pub fn user_function_params(&self) -> Vec<(String, Vec<ParamShape>)> {
        self.functions
            .iter()
            .filter(|(_, def)| def.closure_site.is_none())
            .map(|(name, def)| {
                let params = def
                    .params
                    .iter()
                    .map(|p| (p.name.clone(), p.by_ref, p.variadic))
                    .collect();
                (name.clone(), params)
            })
            .collect()
    }

    /// `get_included_files()`: the main script (when it is a file) and every
    /// file loaded since, in load order.
    pub fn included_files(&self) -> &[String] {
        &self.included_files
    }

    /// Where the next run-time compilation continues numbering.
    pub fn set_compile_counters(&mut self, c: crate::compiler::Counters) {
        self.compile_counters = c;
    }

    /// Append a run-time compilation's `try` table; its ids were numbered to
    /// follow the ones already loaded.
    pub fn append_try_defs(&mut self, defs: Vec<TryDef>) {
        self.try_defs.extend(defs);
    }
}

/// Find the file an `include "name"` means: an absolute path as is; `./` and
/// `../` against the working directory; anything else in each `include_path`
/// directory, then in the directory of the file doing the including.
fn resolve_include(name: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(name);
    if p.is_absolute() || name.starts_with("./") || name.starts_with("../") {
        return p.to_path_buf();
    }
    for dir in INCLUDE_PATH.split(':') {
        let candidate = std::path::Path::new(dir).join(name);
        if candidate.is_file() {
            return candidate;
        }
    }
    let here = with_host(|h| h.current_file().to_string());
    if let Some(dir) = std::path::Path::new(&here)
        .parent()
        .filter(|_| here.starts_with('/'))
    {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return candidate;
        }
    }
    p.to_path_buf()
}

/// `include`/`require`/`include_once`/`require_once`.
///
/// A file that cannot be read warns `Failed to open stream`; `include` then
/// warns `Failed opening … for inclusion` and yields `false`, while `require`
/// throws `Error: Failed opening required …`. A `_once` form yields `true` for a
/// file already loaded. Otherwise the file is compiled and run IN THE CURRENT
/// FRAME, and the construct yields its `return` value, or `1`.
pub fn run_include(kind: crate::ast::IncludeKind, path: &Value) -> Result<Value, String> {
    let name = to_str_ext(path);
    let construct = kind.name();
    if name.is_empty() {
        return Err(crate::builtins::throws_bare(
            "ValueError",
            "Path must not be empty",
        ));
    }
    let target = resolve_include(&name);
    let read = if target.is_dir() {
        // What the reference's `open(2)` + `read(2)` of a directory reports.
        Err(std::io::Error::from_raw_os_error(0))
    } else {
        std::fs::read(&target)
    };
    let src = match read {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => {
            let shown = if target.is_dir() {
                std::fs::canonicalize(&target)
                    .map_or_else(|_| name.clone(), |p| p.display().to_string())
            } else {
                name.clone()
            };
            let reason = match e.raw_os_error() {
                Some(0) => "Undefined error: 0".to_string(),
                _ => {
                    let s = e.to_string();
                    s.find(" (os error")
                        .map_or(s.clone(), |i| s[..i].to_string())
                }
            };
            with_host(|h| {
                h.warn(format!(
                    "{construct}({shown}): Failed to open stream: {reason}"
                ))
            });
            if kind.required() {
                return Err(crate::builtins::throws_bare(
                    "Error",
                    format!("Failed opening required '{name}' (include_path='{INCLUDE_PATH}')"),
                ));
            }
            with_host(|h| {
                h.warn(format!(
                    "{construct}(): Failed opening '{name}' for inclusion \
                     (include_path='{INCLUDE_PATH}')"
                ))
            });
            return Ok(Value::bool(false));
        }
    };
    let real = std::fs::canonicalize(&target).map_or_else(
        |_| target.display().to_string(),
        |p| p.display().to_string(),
    );
    let seen = with_host(|h| h.included_files.contains(&real));
    if kind.once() && seen {
        return Ok(Value::bool(true));
    }
    with_host(|h| {
        // The stream the reference read the file through.
        h.consume_resource_id();
        if !seen {
            h.included_files.push(real.clone());
        }
    });
    let label = with_host(|h| format!("{construct}({})", h.trace_arg(&Value::str(real.clone()))));
    run_loaded(&src, real, label, false)
}

/// `eval($code)`: compile and run `$code` in the current frame. The code is
/// named `<file>(<line>) : eval()'d code` wherever a file would be, and yields
/// its `return` value, or null.
pub fn run_eval(code: &Value) -> Result<Value, String> {
    let src = to_str_ext(code);
    let file = with_host(|h| {
        format!(
            "{}({}) : eval()'d code",
            h.current_file(),
            h.cur_frame_line()
        )
    });
    run_loaded(&format!("<?php {src}"), file, "eval()".to_string(), true)
}

/// Compile `src` as the file `file` and run it on the current frame.
///
/// The compile runs with the host's script name switched to `file`, so every
/// message the lexer and parser raise — and the `ParseError` a syntax error
/// becomes — names it. A syntax error is thrown as a catchable `ParseError`
/// carrying that file and line, as the reference does.
fn run_loaded(src: &str, file: String, label: String, eval: bool) -> Result<Value, String> {
    let saved = with_host(|h| std::mem::replace(&mut h.script_name, file.clone()));
    let counters = with_host(|h| h.compile_counters);
    let compiled = crate::parser::parse_meta(src)
        .map_err(|e| (e.severity, e.message))
        .and_then(|(stmts, _meta)| {
            crate::compiler::compile_nested(&stmts, counters, &file).map_err(|m| {
                let m = match m.strip_prefix(crate::compiler::COMPILE_FATAL) {
                    Some(body) => body.to_string(),
                    None => m,
                };
                ("Fatal error", m)
            })
        });
    with_host(|h| h.script_name = saved);
    let prog = match compiled {
        Ok(p) => p,
        Err(("Parse error", msg)) => return throw_parse_error(&msg, &file),
        Err((severity, msg)) => {
            with_host(|h| h.fatal(severity, &msg));
            return Err(format!("{severity}:  {msg}"));
        }
    };
    let crate::compiler::Program {
        main,
        functions,
        classes,
        try_defs,
        diags,
        counters,
        fn_sites,
        class_sites,
        ..
    } = prog;
    let arc: std::sync::Arc<str> = std::sync::Arc::from(file.as_str());
    let call_line = with_host(|h| {
        h.set_compile_counters(counters);
        h.load_program(functions);
        h.load_classes(classes);
        h.append_try_defs(try_defs);
        h.record_decl_sites(&file, fn_sites, class_sites);
        let call_line = h.cur_frame_line();
        h.includes.push(IncludeFrame {
            depth: h.scopes.len() - 1,
            file: arc,
            label,
            call_line,
        });
        for d in &diags {
            h.diagnose(d.severity, d.level, d.line, &d.msg);
        }
        call_line
    });
    let r = run_chunk_on(main);
    with_host(|h| {
        h.includes.pop();
        if let Some(scope) = h.scopes.last_mut() {
            scope.line = call_line;
        }
    });
    r?;
    Ok(with_host(|h| match h.signal.take() {
        Some(Signal::Return(v)) => v,
        other => {
            h.signal = other;
            if eval {
                Value::Undef
            } else {
                Value::int(1)
            }
        }
    }))
}

/// Raise a syntax error in loaded code as the `ParseError` the reference
/// throws: the message without its ` in <file> on line <n>` tail, and the file
/// and line from that tail rather than from where the loading construct ran.
fn throw_parse_error(msg: &str, file: &str) -> Result<Value, String> {
    let tail = format!(" in {file} on line ");
    let (message, line) = match msg.rfind(&tail) {
        Some(i) => (
            msg[..i].to_string(),
            msg[i + tail.len()..].trim().parse::<i64>().unwrap_or(0),
        ),
        None => (msg.to_string(), 0),
    };
    let exc = new_object("ParseError", vec![Value::str(message)])?;
    with_host(|h| {
        h.prop_set(&exc, "file", Value::str(file.to_string()));
        h.prop_set(&exc, "line", Value::int(line));
    });
    set_pending_throw(exc);
    Ok(Value::Undef)
}

impl PhpHost {
    /// Every loaded `trait`, for a run-time compilation whose classes use one.
    pub fn trait_defs(&self) -> Vec<(String, ClassDef)> {
        self.classes
            .iter()
            .filter(|(_, d)| d.is_trait)
            .map(|(k, d)| (k.clone(), d.clone()))
            .collect()
    }
}

/// The word a diagnostic names a declared type's kind with.
fn class_kind_word(def: &ClassDef) -> &'static str {
    if def.is_trait {
        "trait"
    } else if def.is_interface {
        "interface"
    } else if def.is_enum {
        "enum"
    } else {
        "class"
    }
}

/// [`class_kind_word`] for a built-in type.
fn type_kind_word(kind: TypeKind) -> &'static str {
    match kind {
        TypeKind::Class => "class",
        TypeKind::Interface => "interface",
        TypeKind::Trait => "trait",
        TypeKind::Enum => "enum",
    }
}

// ── request shutdown ─────────────────────────────────────────────────────────

impl PhpHost {
    /// The live `set_exception_handler` callable, when one is set and this run
    /// is one the reference would call it for.
    fn user_exception_handler(&self) -> Option<Value> {
        if !self.exception_handler_enabled {
            return None;
        }
        self.exception_handlers
            .last()
            .filter(|v| !matches!(v, Value::Undef))
            .cloned()
    }

    /// `set_exception_handler($callback)`: push it, answering the one it replaces.
    pub fn set_exception_handler(&mut self, callback: Value) -> Value {
        let prev = self
            .exception_handlers
            .last()
            .cloned()
            .unwrap_or(Value::Undef);
        self.exception_handlers.push(callback);
        prev
    }

    /// `restore_exception_handler()`: drop back to the previous handler.
    pub fn restore_exception_handler(&mut self) {
        self.exception_handlers.pop();
    }

    /// `php -r` code never reaches a user exception handler.
    pub fn disable_exception_handler(&mut self) {
        self.exception_handler_enabled = false;
    }

    /// `register_shutdown_function($callback, ...$args)`.
    pub fn register_shutdown_fn(&mut self, callback: Value, args: Vec<Value>) {
        self.shutdown_fns.push((callback, args));
    }

    /// A fatal error that is not an uncaught exception ended the run.
    pub fn disable_destructors(&mut self) {
        self.destructors_off = true;
    }

    /// How many places other than the containers in `freed` hold `handle` —
    /// the reference count the request-end sweep needs, recovered by looking,
    /// since the heap keeps none. A stale handle in a dead temporary counts,
    /// which errs toward leaving an object to the creation-order pass.
    fn handle_refs(&self, handle: u32, freed: &FxHashSet<u32>) -> usize {
        let is = |v: &Value| matches!(v, Value::Obj(o) if *o == handle);
        let in_frame = |s: &Scope| {
            s.vars
                .slots
                .iter()
                .filter(|slot| matches!(slot, Slot::Val(v) if is(v)))
                .count()
        };
        let mut n = self.scopes.iter().map(in_frame).sum::<usize>()
            + self
                .generators
                .iter()
                .flat_map(|g| g.ctx.frames.iter())
                .map(in_frame)
                .sum::<usize>()
            + self.main_frame_slots.iter().filter(|v| is(v)).count()
            + self.ref_cells.iter().filter(|v| is(v)).count()
            + self.static_props.values().filter(|v| is(v)).count()
            + self.constants.values().filter(|v| is(v)).count();
        for (i, o) in self.objs.iter().enumerate() {
            if freed.contains(&(i as u32)) {
                continue;
            }
            n += match o {
                PhpObj::Array { entries, .. } => entries.values().filter(|v| is(v)).count(),
                PhpObj::Object { props, .. } => props.values().filter(|v| is(v)).count(),
                PhpObj::Closure {
                    captured,
                    bound_this,
                    ..
                } => {
                    captured.iter().filter(|(_, v)| is(v)).count()
                        + usize::from(bound_this.as_ref().is_some_and(is))
                }
                _ => 0,
            };
        }
        n
    }

    /// The handles `handle` itself holds: an array's elements, an object's
    /// properties, a closure's captures and bound `$this`.
    fn held_handles(&self, handle: u32) -> Vec<u32> {
        let vals: Vec<&Value> = match self.objs.get(handle as usize) {
            Some(PhpObj::Array { entries, .. }) => entries.values().collect(),
            Some(PhpObj::Object { props, .. }) => props.values().collect(),
            Some(PhpObj::Closure {
                captured,
                bound_this,
                ..
            }) => captured
                .iter()
                .map(|(_, v)| v)
                .chain(bound_this.iter())
                .collect(),
            _ => Vec::new(),
        };
        vals.into_iter()
            .filter_map(|v| match v {
                Value::Obj(o) => Some(*o),
                _ => None,
            })
            .collect()
    }

    /// The global variables in the order the reference's symbol table holds
    /// them, each with the object handle it holds directly (not through a
    /// reference), newest first.
    fn global_objects_newest_first(&self) -> Vec<(String, u32)> {
        let Some(g) = self.scopes.first() else {
            return Vec::new();
        };
        // The main chunk's names first, in the order it names them; then any
        // bound by name only (`extract`, `$$x`, an include), in binding order.
        let mut order: Vec<&String> = self.main_order.iter().collect();
        let mut rest: Vec<(u32, &String)> = g
            .vars
            .index
            .iter()
            .filter(|(k, _)| !self.main_order.contains(k))
            .map(|(k, &i)| (i, k))
            .collect();
        rest.sort_unstable();
        order.extend(rest.into_iter().map(|(_, k)| k));
        order
            .into_iter()
            .rev()
            .filter_map(|k| {
                let v = match self.main_promoted.iter().position(|p| p == k) {
                    Some(i) => self.main_frame_slots.get(i)?,
                    None => match g.vars.get(k) {
                        Slot::Val(v) => v,
                        _ => return None,
                    },
                };
                match v {
                    Value::Obj(o) if self.is_object_value(v) => Some((k.clone(), *o)),
                    _ => None,
                }
            })
            .collect()
    }

    /// Remove the global `name` from wherever it lives.
    fn drop_global(&mut self, name: &str) {
        match self.main_promoted.iter().position(|p| p == name) {
            Some(i) => {
                if let Some(v) = self.main_frame_slots.get_mut(i) {
                    *v = Value::Undef;
                }
            }
            None => {
                if let Some(g) = self.scopes.first_mut() {
                    if let Some(i) = g.vars.slot_of(name) {
                        g.vars.put(i, Slot::Unset);
                    }
                }
            }
        }
    }

    /// Record the main chunk's global layout (see `PhpHost::main_order`).
    pub fn set_main_layout(&mut self, order: Vec<String>, promoted: Vec<String>) {
        self.main_order = order;
        self.main_promoted = promoted;
        self.main_frame_slots.clear();
    }

    /// The class to run `__destruct` on for `handle`, if it still needs running.
    fn pending_destructor(&self, handle: u32) -> Option<String> {
        if self.destructed.contains(&handle) {
            return None;
        }
        match self.objs.get(handle as usize) {
            Some(PhpObj::Object { class, .. }) if self.class_has_method(class, "__destruct") => {
                Some(class.clone())
            }
            _ => None,
        }
    }
}

/// Request shutdown, in `php_request_shutdown`'s order: the
/// `register_shutdown_function` queue (one registered while it runs is run
/// too), then the destructors (see `call_destructors`). An exception either
/// lets escape is reported uncaught and ends the sweep, as a bailout would.
///
/// An `exit` status already set is parked across the sweep, or every callback
/// would see the run unwinding and return at once; one the sweep sets wins.
pub fn request_shutdown(run_ok_or_uncaught: bool) -> Result<(), String> {
    let exit = with_host(|h| h.pending_exit.take());
    // Nothing calls these from PHP source: a trace names their call site
    // `[internal function]`.
    with_host(|h| {
        if let Some(g) = h.scopes.first_mut() {
            g.internal = true;
        }
    });
    let r = run_shutdown_fns().and_then(|()| {
        if run_ok_or_uncaught && !with_host(|h| h.destructors_off) {
            call_destructors()
        } else {
            Ok(())
        }
    });
    with_host(|h| {
        if h.pending_exit.is_none() {
            h.pending_exit = exit;
        }
    });
    r
}

/// Report an exception a shutdown callback or destructor let escape, the way
/// the top level reports one.
fn shutdown_escape() -> Result<(), String> {
    match with_host(|h| h.pending_throw.take()) {
        Some(exc) => Err(report_uncaught(exc)),
        None => Ok(()),
    }
}

fn run_shutdown_fns() -> Result<(), String> {
    let mut i = 0;
    while let Some((callback, args)) = with_host(|h| h.shutdown_fns.get(i).cloned()) {
        i += 1;
        call_value(callback, args)?;
        with_host(|h| h.signal.take());
        shutdown_escape()?;
        if with_host(|h| h.pending_exit.is_some()) {
            break;
        }
    }
    Ok(())
}

/// `zend_call_destructors`, then `zend_objects_store_call_destructors`.
///
/// First the globals that alone hold an object are freed, newest first, until
/// a pass frees none; freeing one runs its `__destruct`, destroys a suspended
/// generator, and frees in turn whatever only it was holding. Every object
/// still alive after that is destructed in creation order — which, for a
/// generator, is the forced `return` that runs its `finally` blocks.
fn call_destructors() -> Result<(), String> {
    let mut freed: FxHashSet<u32> = FxHashSet::default();
    loop {
        let mut any = false;
        for (name, handle) in with_host(|h| h.global_objects_newest_first()) {
            if freed.contains(&handle) || with_host(|h| h.handle_refs(handle, &freed)) != 1 {
                continue;
            }
            with_host(|h| h.drop_global(&name));
            free_object(handle, &mut freed)?;
            any = true;
        }
        if !any {
            break;
        }
    }
    let count = with_host(|h| h.objs.len()) as u32;
    for handle in 0..count {
        if !freed.contains(&handle) {
            destruct_one(handle)?;
        }
    }
    Ok(())
}

/// Free `handle`: destruct it, then free every object it held that nothing
/// else (outside the freed set) still holds.
fn free_object(handle: u32, freed: &mut FxHashSet<u32>) -> Result<(), String> {
    if !freed.insert(handle) {
        return Ok(());
    }
    destruct_one(handle)?;
    for child in with_host(|h| h.held_handles(handle)) {
        if !freed.contains(&child) && with_host(|h| h.handle_refs(child, freed)) == 0 {
            free_object(child, freed)?;
        }
    }
    Ok(())
}

/// Run `handle`'s destructor once: `__destruct` for an instance, the forced
/// `return` for a suspended generator.
fn destruct_one(handle: u32) -> Result<(), String> {
    if let Some(id) = with_host(|h| match h.objs.get(handle as usize) {
        Some(PhpObj::Generator { id }) => Some(*id),
        _ => None,
    }) {
        gen_destroy(id)?;
        return shutdown_escape();
    }
    let Some(class) = with_host(|h| {
        let c = h.pending_destructor(handle);
        if c.is_some() {
            h.destructed.insert(handle);
        }
        c
    }) else {
        return Ok(());
    };
    call_method(&class, "__destruct", Some(Value::Obj(handle)), Vec::new())?;
    with_host(|h| h.signal.take());
    shutdown_escape()
}

// ── set_error_handler ────────────────────────────────────────────────────────

thread_local! {
    /// Whether [`PhpHost::queued_errors`] may be non-empty — read on every
    /// builtin call, so it is a flag beside the host rather than a borrow of it.
    static ERRORS_QUEUED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

impl PhpHost {
    /// `set_error_handler($callback, $error_levels)`: push it, answering the
    /// callback it replaces.
    pub fn set_error_handler(&mut self, callback: Value, levels: i64) -> Value {
        let prev = self
            .error_handlers
            .last()
            .map(|(c, _)| c.clone())
            .unwrap_or(Value::Undef);
        self.error_handlers.push((callback, levels));
        prev
    }

    /// `restore_error_handler()`: drop back to the previous handler.
    pub fn restore_error_handler(&mut self) {
        self.error_handlers.pop();
    }

    /// `error_reporting()` as PHP code reads it: narrowed to the fatal levels
    /// inside an `@` region.
    pub fn error_reporting_visible(&self) -> i64 {
        if self.suppress > 0 {
            self.error_reporting & errlevel::SILENCED
        } else {
            self.error_reporting
        }
    }
}

/// Hand every queued diagnostic to the live error handler, as
/// `$handler($errno, $errstr, $errfile, $errline)`. A handler that returns
/// `false` gets the default display instead; one that throws stops the drain,
/// and whatever is still queued is dropped with the statement that raised it.
/// Answers whether the caller has to halt its chunk: the handler threw, exited
/// or failed.
///
/// Called at every builtin boundary (before and after the handler runs) and at
/// the end of the main chunk, which is the earliest point the host borrow that
/// raised the diagnostic has been released.
pub fn drain_error_handlers() -> bool {
    if !ERRORS_QUEUED.with(|q| q.get()) {
        return false;
    }
    loop {
        let next = with_host(|h| {
            if h.queued_errors.is_empty() {
                ERRORS_QUEUED.with(|q| q.set(false));
                return None;
            }
            let item = h.queued_errors.remove(0);
            let handler = h.error_handlers.last().map(|(c, _)| c.clone());
            Some((item, handler))
        });
        let Some(((severity, level, msg, file, line), handler)) = next else {
            return false;
        };
        let Some(handler) = handler.filter(|c| !matches!(c, Value::Undef)) else {
            with_host(|h| h.diagnose_default(&severity, level, line, &msg));
            continue;
        };
        let r = invoke_error_handler(handler, level, &msg, file, line);
        if unwinding() {
            with_host(|h| h.queued_errors.clear());
            ERRORS_QUEUED.with(|q| q.set(false));
            return true;
        }
        match r {
            Ok(Value::Bool(false)) => {
                with_host(|h| h.diagnose_default(&severity, level, line, &msg))
            }
            Ok(_) => {}
            Err(e) => {
                with_host(|h| {
                    h.queued_errors.clear();
                    h.set_error(e);
                });
                ERRORS_QUEUED.with(|q| q.set(false));
                return true;
            }
        }
    }
}

/// Call `handler` as the reference calls an error handler:
/// `($errno, $errstr, $errfile, $errline)`, from the line that raised the
/// diagnostic (the call site its trace frame names), with the handler itself
/// unset for the duration so its own diagnostics take the default path.
pub fn invoke_error_handler(
    handler: Value,
    level: i64,
    msg: &str,
    file: String,
    line: u32,
) -> Result<Value, String> {
    with_host(|h| {
        h.in_error_handler = true;
        if let Some(s) = h.scopes.last_mut() {
            s.line = line;
        }
    });
    let args = vec![
        Value::int(level),
        Value::str(msg.to_string()),
        Value::str(file),
        Value::int(i64::from(line)),
    ];
    let r = call_value(handler, args);
    with_host(|h| {
        h.in_error_handler = false;
        h.signal.take();
    });
    r
}

impl PhpHost {
    /// The live error handler, when it takes diagnostics of `level`.
    pub fn error_handler_for(&self, level: i64) -> Option<Value> {
        if self.in_error_handler {
            return None;
        }
        self.error_handlers
            .last()
            .filter(|(c, mask)| !matches!(c, Value::Undef) && mask & level != 0)
            .map(|(c, _)| c.clone())
    }
}

/// Answer a magic method — `__debugInfo`, `__serialize` — for every object
/// reachable from `roots` whose class defines it, keyed by object handle.
///
/// A renderer that consults such a method (`var_dump`, `print_r`,
/// `serialize`) runs inside a host borrow and cannot call PHP code, so the
/// reachable values are walked first, depth-first in render order, and each
/// object's answer is collected here. `answer` shapes what the method returned
/// into the array the renderer will use, or answers `None` having left an
/// exception pending. The walk descends into an answer rather than into the
/// object's own properties, as the render will. `None` when a method threw.
pub fn collect_object_hooks(
    roots: &[Value],
    method: &str,
    answer: &dyn Fn(&str, Value) -> Option<Value>,
) -> Option<FxHashMap<u32, Value>> {
    fn walk(
        v: &Value,
        method: &str,
        answer: &dyn Fn(&str, Value) -> Option<Value>,
        map: &mut FxHashMap<u32, Value>,
        seen: &mut FxHashSet<u32>,
    ) -> bool {
        let Value::Obj(id) = v else { return true };
        if !seen.insert(*id) {
            return true;
        }
        let class = with_host(|h| h.object_class(v).filter(|c| h.class_has_method(c, method)));
        let target = match class {
            Some(class) => {
                let r = call_method(&class, method, Some(v.clone()), Vec::new());
                if unwinding() {
                    return false;
                }
                let Some(arr) = answer(&class, r.unwrap_or(Value::Undef)) else {
                    return false;
                };
                map.insert(*id, arr.clone());
                arr
            }
            None => v.clone(),
        };
        let children: Vec<Value> = with_host(|h| {
            if h.is_object(&target) {
                h.object_props(&target)
                    .into_iter()
                    .map(|(_, x)| x)
                    .collect()
            } else {
                h.array_pairs(&target)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(_, x)| x)
                    .collect()
            }
        });
        children.iter().all(|c| walk(c, method, answer, map, seen))
    }
    let mut map = FxHashMap::default();
    let mut seen = FxHashSet::default();
    roots
        .iter()
        .all(|r| walk(r, method, answer, &mut map, &mut seen))
        .then_some(map)
}

/// `__phplang_try_call($fn)`: run the closure and answer `[true, $result]`,
/// or `[false, $exception]` with the exception it threw taken off the stack.
/// The prelude's stand-in for a `try`/`catch (Throwable)`, which prelude
/// source cannot hold — its try-defs are not loaded beside the program's.
pub fn try_call(f: Value) -> Result<Value, String> {
    let r = call_value(f, Vec::new());
    let thrown = with_host(|h| h.pending_throw.take());
    let pair = match thrown {
        Some(exc) => vec![Value::bool(false), exc],
        None => vec![Value::bool(true), r?],
    };
    Ok(with_host(|h| {
        let arr = h.new_array();
        for v in pair {
            h.arr_push_auto(&arr, v);
        }
        arr
    }))
}

/// The line of the innermost frame running USER code, when PHP-written
/// prelude methods sit above it: the call site an internal method's
/// diagnostics name. `None` when the running frame is user code itself.
fn prelude_call_site_line(h: &PhpHost) -> Option<u32> {
    let top = h.scopes.len() - 1;
    let mut i = top;
    while i > 0 && h.scopes[i].name.as_deref().is_some_and(is_prelude_frame) {
        i -= 1;
    }
    (i < top).then(|| h.scopes[i].line)
}

/// Port of `spl_offset_convert_to_ulong` (`ext/spl/spl_fixedarray.c`) for the
/// prelude's `SplFixedArray`: the integer an offset names. An int passes, a
/// canonical integer string converts, a bool is 0/1, and a float goes through
/// `zend_dval_to_lval_safe` — the cast's `not representable` warning, then the
/// `loses precision` deprecation for a fractional value that fits. Anything
/// else is `zend_illegal_container_offset`'s `TypeError`. Diagnostics name the
/// user's call site. A negative result is the caller's out-of-range case.
pub fn spl_offset(offset: &Value) -> Result<Value, String> {
    let v = with_host(|h| h.deref(offset.clone()));
    let n = match &v {
        Value::Int(n) => Some(*n),
        Value::Str(s) => canonical_int_key(s),
        Value::Bool(b) => Some(*b as i64),
        Value::Float(f) => {
            let f = *f;
            let saved = warn_line();
            if let Some(line) = with_host(|h| prelude_call_site_line(h)) {
                set_warn_line(line);
            }
            let l = with_host(|h| {
                let l = h.to_int_cast(&v);
                const TWO_63: f64 = 9223372036854775808.0;
                let fits = !(f >= TWO_63 || f < -TWO_63);
                if l as f64 != f && fits {
                    let shown = h.to_str(&v);
                    h.deprecated(format!(
                        "Implicit conversion from float {shown} to int loses precision"
                    ));
                }
                l
            });
            set_warn_line(saved);
            Some(l)
        }
        _ => None,
    };
    match n {
        Some(n) => Ok(Value::int(n)),
        None => {
            let ty = with_host(|h| crate::stdlib::types::debug_type(h, &v));
            throw_as_internal_method(
                "TypeError",
                &format!("Cannot access offset of type {ty} on SplFixedArray"),
            )
        }
    }
}

/// Raise `class(message)` from a PHP-written prelude method as the reference
/// raises it from an internal one: the method's own frame is the top of the
/// trace (`#0 file(3): DateTime->__construct('x')`), and the exception's file
/// and line are its CALL SITE's — an internal method has no line of its own,
/// while the prelude method's frame is executing prelude source.
pub fn throw_as_internal_method(class: &str, message: &str) -> Result<Value, String> {
    let exc = new_object(class, vec![Value::str(message.to_string())])?;
    with_host(|h| {
        let n = h.scopes.len();
        if n >= 2 {
            let line = h.scopes[n - 2].line;
            let file = h.scope_file(n - 2).to_string();
            h.prop_set(&exc, "line", Value::int(line as i64));
            h.prop_set(&exc, "file", Value::str(file));
        }
    });
    set_pending_throw(exc);
    Ok(Value::Undef)
}

/// `__phplang_warn`: an E_WARNING raised by prelude code, at the line of the
/// innermost frame running user code — where the reference, whose method is
/// internal, reports it.
pub fn warn_from_prelude(message: &str) {
    let saved = warn_line();
    if let Some(line) = with_host(|h| prelude_call_site_line(h)) {
        set_warn_line(line);
    }
    with_host(|h| h.warn(message));
    set_warn_line(saved);
}

/// Raise a warning from a PHP-written prelude method at its CALL SITE — see
/// [`throw_as_internal_method`].
pub fn warn_as_internal_method(message: &str) {
    let caller = with_host(|h| {
        let n = h.scopes.len();
        (n >= 2).then(|| h.scopes[n - 2].line)
    });
    let saved = warn_line();
    if let Some(line) = caller {
        set_warn_line(line);
    }
    with_host(|h| h.warn(message));
    set_warn_line(saved);
}

/// The classes a declared type's `self`, `parent` and `static` stand for.
#[derive(Debug, Clone, Default)]
pub struct TypeScope {
    pub self_class: Option<String>,
    pub parent: Option<String>,
    pub static_class: Option<String>,
}

impl TypeScope {
    /// The scope of the function a frame name (`Class::method`) belongs to.
    pub fn of_frame(frame: &str, static_class: Option<String>) -> TypeScope {
        let self_class = frame.split_once("::").map(|(c, _)| c.to_string());
        let parent = self_class
            .as_ref()
            .and_then(|c| with_host(|h| h.class_parent(c)));
        TypeScope {
            self_class,
            parent,
            static_class,
        }
    }
}

/// Whether `v` satisfies one alternative of a declared type as it stands —
/// the `zend_check_type_slow` half that converts nothing.
fn type_part_accepts(v: &Value, part: &str, scope: &TypeScope) -> bool {
    let lower = part.to_ascii_lowercase();
    if lower == "mixed" {
        return true;
    }
    let (is_array, obj_class, is_closure) = with_host(|h| {
        let arr = h.is_array(v);
        let cls = if arr { None } else { h.instance_class(v) };
        (arr, cls, h.is_closure(v))
    });
    let instance_of = |class: &str, target: &str| {
        with_host(|h| h.is_a_class(&class.to_ascii_lowercase(), &target.to_ascii_lowercase()))
    };
    match (v, lower.as_str()) {
        (Value::Undef, "null") => true,
        (Value::Bool(_), "bool") => true,
        (Value::Bool(true), "true") | (Value::Bool(false), "false") => true,
        (Value::Int(_), "int") | (Value::Float(_), "float") | (Value::Str(_), "string") => true,
        (Value::Str(_), "callable") => crate::stdlib::callable::callable_reason(v).is_none(),
        (Value::Obj(_), _) if is_array => match lower.as_str() {
            "array" | "iterable" => true,
            "callable" => crate::stdlib::callable::callable_reason(v).is_none(),
            _ => false,
        },
        (Value::Obj(_), _) => {
            let Some(class) = obj_class else { return false };
            match lower.as_str() {
                "object" => true,
                "iterable" => instance_of(&class, "Traversable"),
                "callable" => is_closure || crate::stdlib::callable::callable_reason(v).is_none(),
                "self" => scope
                    .self_class
                    .as_deref()
                    .is_some_and(|s| instance_of(&class, s)),
                "parent" => scope
                    .parent
                    .as_deref()
                    .is_some_and(|s| instance_of(&class, s)),
                "static" => scope
                    .static_class
                    .as_deref()
                    .or(scope.self_class.as_deref())
                    .is_some_and(|s| instance_of(&class, s)),
                "array" | "int" | "float" | "string" | "bool" | "null" | "false" | "true"
                | "void" | "never" => false,
                _ => part
                    .split('&')
                    .all(|t| instance_of(&class, t.trim_start_matches('\\'))),
            }
        }
        _ => false,
    }
}

/// `zend_verify_weak_scalar_type_hint`: the coercive conversions a union
/// tries, in its order — int, float, string, bool.
fn weak_scalar(h: &mut PhpHost, v: &Value, has: &dyn Fn(&str) -> bool) -> Option<Value> {
    fn fits(f: f64) -> bool {
        f.is_finite() && (-9223372036854775808.0..9223372036854775808.0).contains(&f)
    }
    if has("int") {
        if has("float") && matches!(v, Value::Str(_)) {
            if let Some(n) = parse_php_number_full(&h.to_str(v)) {
                return Some(n);
            }
        } else {
            // `zend_parse_arg_long_weak`.
            let long = match v {
                Value::Float(f) if f.is_nan() || !fits(*f) => None,
                Value::Float(f) => {
                    if f.fract() != 0.0 {
                        let shown = h.to_str(v);
                        h.deprecated(format!(
                            "Implicit conversion from float {shown} to int loses precision"
                        ));
                    }
                    Some(*f as i64)
                }
                Value::Str(s) => match parse_php_number_full(s) {
                    Some(Value::Int(i)) => Some(i),
                    Some(n) => {
                        let f = n.to_float();
                        if !fits(f) {
                            None
                        } else {
                            if f.fract() != 0.0 {
                                h.deprecated(format!(
                                    "Implicit conversion from float-string \"{s}\" to int loses precision"
                                ));
                            }
                            Some(f as i64)
                        }
                    }
                    None => None,
                },
                Value::Bool(b) => Some(i64::from(*b)),
                _ => None,
            };
            if let Some(l) = long {
                return Some(Value::Int(l));
            }
        }
    }
    if has("float") {
        let d = match v {
            Value::Int(i) => Some(*i as f64),
            Value::Str(s) => parse_php_number_full(s).map(|n| n.to_float()),
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            _ => None,
        };
        if let Some(d) = d {
            return Some(Value::Float(d));
        }
    }
    // PHP 8.5's weak string and bool parses warn on a NaN: the value converts,
    // but `"NAN"`/`true` do not read back as the number it was.
    if has("string") && matches!(v, Value::Int(_) | Value::Float(_) | Value::Bool(_)) {
        return Some(Value::str(h.to_str_diag(v)));
    }
    if has("bool") && matches!(v, Value::Int(_) | Value::Float(_) | Value::Str(_)) {
        if matches!(v, Value::Float(f) if f.is_nan()) {
            h.warn("unexpected NAN value was coerced to bool");
        }
        return Some(Value::bool(h.is_truthy(v)));
    }
    None
}

/// `zend_check_type` + `zend_verify_scalar_type_hint`: `v` against a declared
/// type — classes (with `self`/`parent`/`static`), `array`, `callable`,
/// `iterable`, `object`, `mixed`, `true`/`false`/`null`, unions and
/// intersections. A value that does not satisfy it as it stands is converted
/// when the mode is coercive and a scalar alternative takes it, in the
/// reference's order; under `strict_types` only int widens to float. `Err`
/// carries the type the value had, for the `TypeError`.
pub fn verify_type(v: Value, ty: &TypeHint, scope: &TypeScope) -> Result<Value, String> {
    if ty.parts.iter().any(|p| type_part_accepts(&v, p, scope)) {
        return Ok(v);
    }
    let has = |want: &str| ty.parts.iter().any(|p| p.eq_ignore_ascii_case(want));
    let strict = with_host(|h| h.strict_types);
    if strict {
        if let (Value::Int(i), true) = (&v, has("float")) {
            return Ok(Value::Float(*i as f64));
        }
        return Err(with_host(|h| h.type_name_for_error(&v)));
    }
    // An object reaches a `string` alternative through `__toString`, the one
    // conversion `zend_parse_arg_str_weak` makes for something not scalar.
    if has("string") && with_host(|h| h.is_object(&v)) {
        let stringable = with_host(|h| {
            h.object_class(&v)
                .is_some_and(|c| h.class_has_method(&c.to_ascii_lowercase(), "__tostring"))
        });
        if stringable {
            return Ok(Value::str(to_str_ext(&v)));
        }
    }
    with_host(|h| match weak_scalar(h, &v, &has) {
        Some(converted) => Ok(converted),
        None => Err(h.type_name_for_error(&v)),
    })
}

/// A declared type as the reference prints it in a `TypeError` —
/// `zend_type_to_string`, with `self`/`parent` resolved against the frame's
/// class.
fn render_declared(ty: &TypeHint, frame: &str, static_class: Option<&str>) -> String {
    let (self_class, parent) = match frame.split_once("::") {
        Some((c, _)) => with_host(|h| {
            let shown = h.class_display_name(&c.to_ascii_lowercase());
            let parent = h
                .class_parent(c)
                .map(|p| h.class_display_name(&p.to_ascii_lowercase()));
            (shown, parent)
        }),
        None => ("self".to_string(), None),
    };
    // `static` is reported as the class it stood for in the call.
    let mut ty = ty.clone();
    if let Some(s) = static_class {
        let shown = with_host(|h| h.class_display_name(&s.to_ascii_lowercase()));
        for p in &mut ty.parts {
            if p.eq_ignore_ascii_case("static") {
                *p = shown.clone();
            }
        }
    }
    ty.declared(&self_class, parent.as_deref())
}

/// The callee a call-time refusal is raised for — see [`with_callee`].
#[derive(Clone)]
struct CalleeCtx {
    this: Option<Value>,
    site: Option<DeclSite>,
}

thread_local! {
    static CALLEE: RefCell<Option<CalleeCtx>> = const { RefCell::new(None) };
}

/// Run a call's argument or return check with its callee recorded, so a
/// refusal's frame reads `A->m(...)` / `{closure:f.php:3}(...)` as the
/// reference's does.
fn with_callee<R>(ctx: &CalleeCtx, f: impl FnOnce() -> R) -> R {
    let saved = CALLEE.with(|c| c.replace(Some(ctx.clone())));
    let r = f();
    CALLEE.with(|c| *c.borrow_mut() = saved);
    r
}

/// The callee's name as a refusal's message spells it: `A::m`, `f`, or
/// `{closure:f.php:3}` (class-prefixed with `::` for a closure in a class).
fn callee_shown(frame: &str) -> String {
    let site = CALLEE.with(|c| c.borrow().as_ref().and_then(|x| x.site.clone()));
    let Some(DeclSite::Closure(inner, line)) = site else {
        return display_frame(frame);
    };
    let script = with_host(|h| h.script_name().to_string());
    let rendered = format!("{{closure:{}:{line}}}", inner.render(&script));
    match frame.split_once("::") {
        Some((class, _)) => {
            let shown = with_host(|h| h.class_display_name(&class.to_ascii_lowercase()));
            format!("{shown}::{rendered}")
        }
        None => rendered,
    }
}

/// [`throw_from_internal`] for a refusal of a USER function's call: the pushed
/// frame carries the callee's `$this` and closure site.
fn throw_from_callee(
    func: &str,
    args: &[Value],
    class: &str,
    message: &str,
) -> Result<Value, String> {
    let ctx = CALLEE.with(|c| c.borrow().clone());
    let Some(ctx) = ctx else {
        return throw_from_internal(func, args, class, message);
    };
    with_host(|h| {
        let line = h.cur_frame_line();
        h.scopes.push(Scope {
            name: Some(func.to_string()),
            line,
            internal: true,
            closure_site: ctx.site.clone(),
            ..Scope::default()
        });
        let argsarr = h.new_array();
        for a in args {
            h.arr_push_auto(&argsarr, a.clone());
        }
        h.set_var("@args", argsarr);
        if let Some(this) = &ctx.this {
            h.set_var("this", this.clone());
        }
    });
    let exc = new_object(class, vec![Value::str(message.to_string())]);
    with_host(|h| {
        h.scopes.pop();
    });
    set_pending_throw(exc?);
    Ok(Value::Undef)
}

/// Whether a prelude frame named `frame` is one of the prelude's own helpers
/// rather than a method of the class it stands in for: a class whose name
/// starts with `__`, or a `__`-named method that is no PHP magic method. A
/// prelude `__call` is a helper too: it forwards to an inner iterator the way
/// `spl_dual_it_get_method` does, which the reference does with no frame.
fn is_prelude_helper_name(frame: &str) -> bool {
    const MAGIC: &[&str] = &[
        "__construct",
        "__destruct",
        "__callstatic",
        "__get",
        "__set",
        "__isset",
        "__unset",
        "__tostring",
        "__invoke",
        "__set_state",
        "__clone",
        "__debuginfo",
        "__serialize",
        "__unserialize",
        "__sleep",
        "__wakeup",
    ];
    let Some((class, method)) = frame.split_once("::") else {
        return false;
    };
    class.starts_with("__")
        || (method.starts_with("__") && !MAGIC.contains(&method.to_ascii_lowercase().as_str()))
}

/// `zend_deprecated_function`: `Method K::m() is deprecated` or `Function f()
/// is deprecated`, then ` since <since>` and `, <message>` from the attribute.
/// A user function's is `E_USER_DEPRECATED`; a prelude method stands in for an
/// internal one, whose is `E_DEPRECATED`.
fn raise_deprecated_call(frame: &str, d: &crate::ast::Deprecation) {
    let kind = if frame.contains("::") {
        "Method"
    } else {
        "Function"
    };
    let mut msg = format!("{kind} {}() is deprecated", display_frame(frame));
    if let Some(since) = &d.since {
        msg.push_str(&format!(" since {since}"));
    }
    if let Some(message) = &d.message {
        msg.push_str(&format!(", {message}"));
    }
    let level = if is_prelude_frame(frame) {
        errlevel::E_DEPRECATED
    } else {
        errlevel::E_USER_DEPRECATED
    };
    with_host(|h| h.diagnose("Deprecated", level, warn_line(), msg));
}
