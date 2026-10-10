//! Lower the PHP AST to `fusevm::Chunk`.
//!
//! Arithmetic `+ - *` lowers to native fusevm ops so the JIT can trace them; the
//! strict numeric hook (host) supplies PHP coercion for non-numeric operands.
//! `/ % **`, string concat, comparisons, and everything PHP-specific — variable
//! access, arrays, function dispatch — lower to a `CallBuiltin` that lands in
//! `builtins.rs`. Conditions are normalized through the `TRUTHY` builtin before a
//! native jump, because PHP truthiness (`0`, `""`, `"0"`, `[]`, `null` are falsy)
//! differs from fusevm's default numeric truthiness.

use crate::ast::*;
use crate::host::{self, ops, CatchClause, ClassDef, FuncDef, TryDef};
use crate::lexer::CompileDiag;
use fusevm::{Chunk, ChunkBuilder, Op, Value};
use rustc_hash::{FxHashMap, FxHashSet};

/// What a class declares `final`, gathered from its own members and the
/// traits it uses, plus where each of its methods was declared — the halves
/// of [`ClassDef`] that [`host::final_violation`] reads.
#[derive(Default)]
struct Finals {
    methods: FxHashSet<String>,
    consts: FxHashSet<String>,
    props: FxHashSet<String>,
    method_sites: Vec<(String, u32)>,
}

/// Why a declaration the compiler could read cannot be LINKED, and in which of
/// the reference's two shapes it says so.
enum LinkError {
    /// A bare fatal error: displayed with a stack trace, but raised below the
    /// exception machinery, so no `try`/`catch` can see it.
    Fatal(String),
    /// The message of an ordinary throwable `Error`.
    Throw(String),
}

/// The marker a compile-time `Fatal error` raised by the COMPILER carries at the
/// head of its `Err`, so the entry points can display it in PHP's shape (the
/// parser marks its own the same way). A control character, so no message text
/// can forge one.
pub const COMPILE_FATAL: char = '\u{1}';

/// Where a type this compilation declares was declared, as a redeclaration
/// quotes it.
#[derive(Clone)]
struct ClassSite {
    kind: &'static str,
    name: String,
    line: u32,
    namespace: String,
}

/// The word a redeclaration names a declaration's kind with.
fn decl_kind_word(d: &ClassDecl) -> &'static str {
    if d.is_trait {
        "trait"
    } else if d.is_interface {
        "interface"
    } else if d.is_enum {
        "enum"
    } else {
        "class"
    }
}

/// The full output of compiling a program.
pub struct Program {
    pub main: Chunk,
    /// The global frame's variables in the order the main chunk numbers them.
    /// Reserved before the chunk runs, so slot `n` in it and slot `n` in the
    /// global frame are the same variable. Another chunk that later runs in the
    /// same frame — an `include`, an `eval`, a parameter default — addresses
    /// variables by name and so reaches the same slots without needing an order
    /// of its own.
    pub main_locals: Vec<String>,
    /// Every global the main chunk names, in order of first appearance — the
    /// order the reference's symbol table holds them in, which is the order the
    /// request-end destructor pass walks (backwards).
    pub main_order: Vec<String>,
    /// The globals held in fusevm frame slots rather than the host scope,
    /// indexed by their slot (see `crate::promote`).
    pub main_promoted: Vec<String>,
    pub functions: Vec<(String, FuncDef)>,
    pub classes: Vec<(String, ClassDef)>,
    /// `try`/`catch`/`finally` constructs, indexed by the id baked into each
    /// `RUN_TRY` call.
    pub try_defs: Vec<TryDef>,
    /// Notices raised while READING this source (see [`CompileDiag`]). They are
    /// emitted once, before the first instruction runs — carrying them on the
    /// program rather than in a global is what guarantees the ordering, since the
    /// prelude is compiled after the user's source but must not interleave.
    pub diags: Vec<CompileDiag>,
    /// Where this compilation's counters stopped; see [`Counters`].
    pub counters: Counters,
    /// `(lowercased name, line, namespace)` of every function this program
    /// declares at its top level — the site a later redeclaration quotes.
    pub fn_sites: Vec<(String, u32, String)>,
    /// The same for the types it declares at its top level.
    pub class_sites: Vec<(String, u32, String)>,
}

/// The per-compilation counters that mint names and ids — temporaries,
/// `static` storage keys, anonymous-class numbers, `try` ids. Code compiled at
/// run time (`include`, `eval`) continues from where the code already loaded
/// left off, so what it mints never collides with a name already in use in the
/// same frame or the same function table.
#[derive(Debug, Clone, Copy, Default)]
pub struct Counters {
    pub tmp: usize,
    pub static_slot: usize,
    pub anon_classes: usize,
    pub try_defs: usize,
}

/// Break/continue jump fixups for the innermost loop.
struct LoopCtx {
    /// Unique per loop in the compilation — a `goto` label records the loops
    /// around it by these.
    id: usize,
    breaks: Vec<usize>,
    continues: Vec<usize>,
    /// For a `foreach` over a generator whose subject is not a plain variable:
    /// the hidden temporaries holding the subject and the generator mark taken
    /// before it was evaluated. A `return` from inside the loop releases it
    /// (see [`ops::GEN_RELEASE`]) before the frame exits, which is when the
    /// reference frees the loop's iterator and so destroys a generator nothing
    /// else holds.
    gen_subj: Option<(String, String)>,
    /// A `switch`, which `continue` reaches like a loop but warns about.
    is_switch: bool,
}

/// One segment of a flattened array lvalue chain: an explicit `[key]` or an
/// append `[]`.
enum LvSeg<'a> {
    Key(&'a Expr),
    Append,
}

/// The `ops::ARR_MUT` sub-op for a by-reference array mutator name, or `None` if
/// the name isn't one. These lower specially (passing the array by variable
/// name) rather than through the normal `CALL` value path.
fn array_mutator_subop(name: &str) -> Option<i64> {
    use crate::host::arrmut;
    match name.to_ascii_lowercase().as_str() {
        "array_push" => Some(arrmut::PUSH),
        "array_pop" => Some(arrmut::POP),
        "array_shift" => Some(arrmut::SHIFT),
        "array_unshift" => Some(arrmut::UNSHIFT),
        "array_splice" => Some(arrmut::SPLICE),
        _ => None,
    }
}

/// How an argument in a by-reference position can supply the location the
/// parameter writes back to.
///
/// The reference draws this line by the OPCODE that produced the operand, not by
/// its type: an `IS_VAR` result can be bound, an `IS_TMP_VAR` or `IS_CONST`
/// cannot. `Lvalue` is the group that binds silently, `VarTemp` the group that
/// binds to a temporary after a notice, and `TmpConst` the group that is an
/// error.
#[derive(Clone, Copy, PartialEq)]
enum ByRefArg {
    Lvalue,
    VarTemp,
    TmpConst,
}

/// Which group a by-reference argument falls into. See [`ByRefArg`].
///
/// The groups are not guessable from the shape of the syntax, so each was read
/// off the reference:
///
/// * `$$name` is a real location and binds silently, but `@$name` does NOT —
///   the suppression operator makes the result a temporary;
/// * `$o->p` binds, `$o?->p` is an error, because the nullsafe operator is
///   rejected outright in a write context;
/// * `new C` and `new class {}` bind to a temporary with a notice, while
///   `clone $o` — which also yields a fresh object — is an error;
/// * a subscript binds however its BASE was produced, so `mk()[0]` is silent
///   even though `mk()` alone would warn.
fn byref_arg_class(e: &Expr) -> ByRefArg {
    match e {
        // A named argument is judged by the value it carries.
        Expr::NamedArg(_, inner) => byref_arg_class(inner),
        // Real locations. `$$name` is one of them: the name is computed, but
        // what it names is a variable like any other, and the reference binds it
        // silently.
        Expr::Var(_)
        | Expr::VarVar(_)
        | Expr::Index(..)
        | Expr::PropGet(..)
        | Expr::StaticProp(..) => ByRefArg::Lvalue,
        // Calls and instantiations leave a temporary the engine can still bind.
        Expr::Call(..)
        | Expr::CallValue(..)
        | Expr::MethodCall(..)
        | Expr::NullsafeMethodCall(..)
        | Expr::StaticCall(..)
        | Expr::New(..)
        | Expr::NewDyn(..)
        | Expr::NewAnon { .. } => ByRefArg::VarTemp,
        _ => ByRefArg::TmpConst,
    }
}

/// The receiver of `e` when `e` is one LINK of a `->` / `?->` / `[…]` access
/// chain, or `None` when `e` is not a link at all.
///
/// The spine this walks is exactly what a nullsafe operator short-circuits
/// over — see [`chain_has_nullsafe`].
fn chain_recv(e: &Expr) -> Option<&Expr> {
    match e {
        Expr::PropGet(r, _)
        | Expr::NullsafePropGet(r, _)
        | Expr::MethodCall(r, _, _)
        | Expr::NullsafeMethodCall(r, _, _)
        | Expr::Index(r, _) => Some(r),
        _ => None,
    }
}

/// Whether the receiver SPINE of `e` spells a `?->`.
///
/// Only the spine is walked. A `?->` inside an argument or a subscript is its
/// own chain and short-circuits its own extent: `$a->m($n?->x->y)` must skip
/// `->y` and still call `$a->m`, so the two chains cannot share one exit.
fn chain_has_nullsafe(e: &Expr) -> bool {
    let mut cur = e;
    loop {
        if matches!(
            cur,
            Expr::NullsafePropGet(..) | Expr::NullsafeMethodCall(..)
        ) {
            return true;
        }
        match chain_recv(cur) {
            Some(r) => cur = r,
            None => return false,
        }
    }
}

/// The by-reference positions that RAISE a diagnostic, as
/// `(name, 1-based argument number, parameter name)`.
///
/// Being by-reference is not enough to be listed here. `array_multisort`,
/// `extract`, `current` and `key` all take their array by reference and still
/// accept a literal in silence, because their parameters are declared
/// `PREFER_REF` — the engine binds a reference when one is available and falls
/// back to the value when it is not, with nothing to report either way. Adding
/// them would invent diagnostics the reference does not emit.
const BYREF_ARG_DIAG: &[(&str, u32, &str)] = &[
    ("sort", 1, "array"),
    ("rsort", 1, "array"),
    ("asort", 1, "array"),
    ("arsort", 1, "array"),
    ("ksort", 1, "array"),
    ("krsort", 1, "array"),
    ("usort", 1, "array"),
    ("uasort", 1, "array"),
    ("uksort", 1, "array"),
    ("natsort", 1, "array"),
    ("natcasesort", 1, "array"),
    ("shuffle", 1, "array"),
    ("array_push", 1, "array"),
    ("array_pop", 1, "array"),
    ("array_shift", 1, "array"),
    ("array_unshift", 1, "array"),
    ("array_splice", 1, "array"),
    ("array_walk", 1, "array"),
    ("array_walk_recursive", 1, "array"),
    ("end", 1, "array"),
    ("reset", 1, "array"),
    ("next", 1, "array"),
    ("prev", 1, "array"),
    ("settype", 1, "var"),
    ("parse_str", 2, "result"),
    ("similar_text", 3, "percent"),
    ("preg_match", 3, "matches"),
    ("preg_match_all", 3, "matches"),
    ("str_replace", 4, "count"),
    ("str_ireplace", 4, "count"),
    ("preg_replace", 5, "count"),
    ("preg_filter", 5, "count"),
    ("preg_replace_callback", 5, "count"),
    ("preg_replace_callback_array", 4, "count"),
];

/// The highest argument number [`BYREF_ARG_DIAG`] describes. A named argument
/// can reach a slot far past the number of arguments actually written —
/// `preg_replace(count: [])` fills argument 5 with one argument — so the
/// name-keyed lookup asks for every slot rather than only the reachable ones.
const BYREF_MAX_ARGNO: usize = 5;

/// The by-reference position of `name` that a call with `nargs` arguments
/// actually fills, as `(0-based index, 1-based argument number, parameter name)`.
///
/// `sscanf` takes every argument from the third on by reference, and its
/// message names no parameter at all — the variadic tail has no name to print,
/// so the reference writes `Argument #3 could not be passed by reference`
/// without the usual `($name)`.
fn byref_diag_slots(name: &str, nargs: usize) -> Vec<(usize, u32, &'static str)> {
    let lname = name
        .rsplit('\\')
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    if lname == "sscanf" {
        return (2..nargs).map(|i| (i, i as u32 + 1, "")).collect();
    }
    BYREF_ARG_DIAG
        .iter()
        .filter(|(n, argno, _)| *n == lname && (*argno as usize) <= nargs)
        .map(|&(_, argno, param)| (argno as usize - 1, argno, param))
        .collect()
}

/// Whether this call is a literal two-argument `min()`/`max()` — the ONE shape
/// the reference compiles to its frameless implementation of those functions
/// (`ZEND_FRAMELESS_FUNCTION(min, 2)`, `ext/standard/array.c:1282`).
///
/// The distinction is observable: `min(1, NAN)` written out is NAN, while
/// `call_user_func('min', 1, NAN)`, `$f = 'min'; $f(1, NAN)`, `min(...[1, NAN])`
/// and `(min(...))(1, NAN)` are all 1, because none of those reaches the
/// frameless form. Every one of those goes through `Expr::CallValue` or the
/// spread/named arms, so testing the direct arm's name and arity is enough.
fn is_direct_minmax2(name: &str, args: &[Expr]) -> bool {
    if args.len() != 2 {
        return false;
    }
    // `\min(…)` is the same function; phplang folds a qualified name to its last
    // segment, so a leading separator is all that can remain.
    let bare = name.rsplit('\\').next().unwrap_or(name);
    bare.eq_ignore_ascii_case("min") || bare.eq_ignore_ascii_case("max")
}

/// A call's by-reference argument slots: the callee's DECLARED spelling, and
/// one `(0-based index, 1-based argument number, parameter name)` per position
/// the callee writes through. The name is empty for a variadic position, which
/// the reference's message leaves unnamed.
type ByRefSlots = (String, Vec<(usize, u32, String)>);

/// What the pre-pass records about a USER function that takes a parameter BY
/// REFERENCE, which is everything a call site needs to judge its arguments
/// without having compiled the declaration yet.
struct ByRefFn {
    /// The DECLARED spelling: `function Foo(&$a)` called as `FOO(1)` is refused
    /// as `Foo()`.
    spelled: String,
    /// `(position, parameter name)` per by-reference parameter. The name is
    /// empty for a variadic one, which the reference's message leaves unnamed.
    byref: Vec<(usize, String)>,
    /// EVERY parameter name, in order — needed to tell a named argument that
    /// binds somewhere from one that binds nowhere. The reference sends the
    /// arguments in written order, so an unknown name earlier in the call is
    /// reported before a by-reference refusal later in it: `f(b: 2, a: 1)` is
    /// `Unknown named parameter $b` even though `$a` is by reference.
    params: Vec<String>,
    /// Whether the function is variadic, in which case NO name binds nowhere —
    /// the variadic parameter collects the ones no other parameter claims.
    variadic: bool,
}

/// What [`Compiler::enter_scope`] hands back so the enclosing scope can be
/// restored: its host slot map, its slot order, and its promoted-local map.
type SavedScope = (
    FxHashMap<String, u32>,
    Vec<String>,
    FxHashMap<String, u16>,
    FxHashSet<String>,
);

#[derive(Default)]
pub struct Compiler {
    functions: Vec<(String, FuncDef)>,
    classes: Vec<(String, ClassDef)>,
    /// The class currently being compiled, and its parent — used to resolve
    /// `self::`/`parent::`/`static::` to concrete class names.
    current_class: Option<String>,
    current_parent: Option<String>,
    loops: Vec<LoopCtx>,
    /// Compiled `try`/`catch`/`finally` bodies; a `RUN_TRY` call references an
    /// entry by its index here.
    try_defs: Vec<TryDef>,
    /// Nesting depth of `try`/`catch`/`finally` bodies currently being lowered.
    /// While `> 0` with no loop in the same detached chunk, `break`/`continue`
    /// lower to control signals the orchestrator relays, not in-chunk jumps.
    in_try: usize,
    /// The loops (and `switch`es) enclosing the current detached `try` chunk
    /// from the chunks around it, outermost first, each marked `true` for a
    /// `switch` — the levels a `break` here can still reach through a control
    /// signal. A function body starts again from none.
    outer_loops: Vec<bool>,
    /// Last [`LoopCtx::id`] handed out.
    loop_seq: usize,
    /// The `goto` labels and jumps of the function being compiled.
    gotos: Gotos,
    /// Which chunk the code being lowered lands in — a `try` body is a chunk
    /// of its own — and the last id handed out.
    cur_chunk: usize,
    chunk_seq: usize,
    /// Monotonic counter for compiler-generated temporary variable names
    /// (`foreach` desugaring), kept out of the PHP identifier space with a `@`.
    tmp: usize,
    /// Monotonic counter minting a stable, unique storage key for each `static
    /// $var` declaration, baked into the chunk so every call resolves the same
    /// persistent slot.
    static_slot: usize,
    /// By-reference parameter positions per function (lowercased name), gathered
    /// in a pre-pass so a call can write the callee's final by-ref values back to
    /// the caller's variables even when the function is declared later.
    byref_fns: FxHashMap<String, Vec<usize>>,
    /// The same pre-pass' by-reference parameters of every USER function, as
    /// `(declared spelling, [(position, parameter name)])`, which is what the
    /// refusal `f(): Argument #1 ($a) could not be passed by reference` needs and
    /// [`Compiler::byref_fns`] does not carry. Kept apart from that map because
    /// it also holds the by-ref BUILTINS, whose refusals come from the
    /// [`BYREF_ARG_DIAG`] table instead, and a name in both would be judged
    /// twice. The spelling is the DECLARED one: `function Foo(&$a)` called as
    /// `FOO(1)` is refused as `Foo()`.
    byref_user_fns: FxHashMap<String, ByRefFn>,
    /// Emit per-statement DAP line markers (`php --dap`). Off for normal runs so
    /// the compiled chunk carries zero extra ops.
    debug: bool,
    /// Set while lowering the body of a `function &f()`, so a `return` naming an
    /// lvalue publishes that storage cell instead of copying its value.
    ret_by_ref: bool,
    /// The `void` or `never` return type of the body being lowered (`None`
    /// for any other type, and for a generator), which every `return` in it is
    /// checked against while compiling.
    ret_rule: Option<&'static str>,
    /// Where the code currently being lowered was WRITTEN, which is what names
    /// a closure literal's stack frames (`{closure:<here>:<line>}`). It follows
    /// the declaration nesting rather than the call nesting: `Script` at the top
    /// level, `Named("K::m()")` inside a method, and a `Closure` link for each
    /// closure literal entered. Saved and restored around every body lowered.
    decl_site: host::DeclSite,
    /// The line of the statement currently being lowered, stamped onto the ops
    /// that can raise a diagnostic so `Warning: … on line N` names it. Expression
    /// granularity would need a line on every AST node; a statement that spans
    /// several lines therefore reports its first.
    cur_line: u32,
    /// Method names of each compiled class in their declared spelling and
    /// declaration order, keyed by the lowercased class name.
    ///
    /// `ClassDef::methods` is a hash map keyed by the *lowercased* name, so it
    /// keeps neither — and a trait-conflict diagnostic needs both: PHP echoes
    /// the method back as the trait spelled it, and which of several collisions
    /// it reports depends on the order the members were declared in.
    method_order: FxHashMap<String, Vec<String>>,
    /// How many anonymous classes have been given a name so far. PHP numbers
    /// them from zero across the whole compilation unit, in source order, and
    /// bakes the number into the generated class name.
    anon_classes: usize,
    /// The traits already loaded when this compilation started, which a class
    /// in `include`d code may `use` without declaring them itself.
    known_traits: Vec<(String, ClassDef)>,
    /// The id the first `try` of this compilation takes: 0 for a program,
    /// the number already loaded for a run-time compilation.
    try_base: usize,
    /// The scope currently being lowered, as name → frame slot, when its
    /// variables were resolved to indices. Empty while lowering a chunk that
    /// runs in a frame it did not seed (a parameter default, an `include`, an
    /// `eval`), which must keep addressing variables by name.
    slots: FxHashMap<String, u32>,
    /// The same names in slot order, handed to the runtime so the frame reserves
    /// its slots in exactly the order the chunk addresses them.
    slot_order: Vec<String>,
    /// The locals of the scope being lowered that live in a fusevm FRAME SLOT
    /// rather than in the host scope, and their slot numbers.
    ///
    /// Chosen by `crate::promote`, which only offers a name it can prove needs
    /// none of the three things the host storage provides — an unset state to
    /// warn about, a shared reference cell, or a lookup by name. Consulted
    /// before [`Compiler::slots`] everywhere a variable is read or written; a
    /// name that is not here keeps the by-name/host-slot path unchanged.
    fslots: FxHashMap<String, u16>,
    /// Whether the declaration being lowered is a `trait`, in which case `self`
    /// and `parent` are resolved at run time rather than baked — see
    /// [`Compiler::emit_class_name`].
    in_trait: bool,
    /// Of [`Compiler::fslots`], the names every write to is provably numeric, so
    /// `++` may be lowered as `+ 1` on the native `Add` rather than through the
    /// host step.
    fnumeric: FxHashSet<String>,
    /// Set for the one statement about to be lowered when it sits at the file's
    /// top level (directly, or in a plain `{ }` or `namespace { }` block), and
    /// taken by `compile_stmt` on entry. A function or type declared there is
    /// bound before the program runs; one declared anywhere else is bound when
    /// its statement runs (`ops::DECLARE_FN`, `ops::DECLARE_CLASS`).
    top_decl: bool,
    /// The top-level functions declared so far: lowercased name → (line,
    /// namespace).
    fn_decls: FxHashMap<String, (u32, String)>,
    /// The top-level types bound so far, in the order the reference binds them.
    class_decls: FxHashMap<String, ClassSite>,
    /// The top-level type declarations (by address) the reference binds EARLY,
    /// at compile time, rather than where they stand — see [`Compiler::early_bind`].
    early: FxHashSet<usize>,
    /// Compiling the PHP-written prelude, whose declarations ARE the built-ins
    /// a user declaration is checked against, so it is not checked itself.
    prelude: bool,
}

/// Compile a parsed program. `debug` enables per-statement DAP line markers.
pub fn compile(stmts: &[Stmt], debug: bool) -> Result<Program, String> {
    compile_program(stmts, debug, false)
}

/// Compile the PHP-written prelude. Its declarations are what a program's own
/// are checked against, so they are not checked themselves.
pub fn compile_prelude(stmts: &[Stmt]) -> Result<Program, String> {
    compile_program(stmts, false, true)
}

fn compile_program(stmts: &[Stmt], debug: bool, prelude: bool) -> Result<Program, String> {
    let mut c = Compiler {
        debug,
        prelude,
        ..Compiler::default()
    };
    // Pre-pass: record by-reference parameter positions of every function so a
    // call site can write the callee's finals back even for forward references.
    c.seed_builtin_byref();
    c.collect_byref(stmts);
    let mut b = ChunkBuilder::new();
    // The top level is a frame like any other, and it is where a script's hot
    // loop usually is, so its locals are offered for promotion too — but a
    // top-level local is also a PHP GLOBAL, which `global $x` and `$GLOBALS`
    // reach from inside any function, by name, through the host scope. Whatever
    // they can reach has to stay there.
    let mut promoted = c.promotable_locals(&[], stmts, false);
    match crate::promote::globals_reached(stmts) {
        Some(reached) => promoted.names.retain(|n| !reached.contains(n)),
        // `$GLOBALS` is mentioned and its subscript may be computed, so no
        // top-level name can be shown to be out of reach.
        None => promoted.names.clear(),
    }
    let main_order = scope_slots(&[], stmts);
    let main_promoted = promoted.names.clone();
    let saved = c.enter_scope_promoting(main_order.clone(), promoted);
    c.compile_top_level(&mut b, stmts)?;
    c.resolve_gotos(&mut b, 0, true)?;
    let main_locals = c.leave_scope(saved);
    let counters = c.counters();
    let (fn_sites, class_sites) = c.decl_sites();
    Ok(Program {
        fn_sites,
        class_sites,
        main: b.build(),
        main_locals,
        main_order,
        main_promoted,
        functions: c.functions,
        classes: c.classes,
        try_defs: c.try_defs,
        counters,
        // Drained here rather than in the lexer's caller: this is the last point
        // that still belongs to compiling THIS source, so no later compilation
        // (the prelude, an `eval`) can inherit or lose them.
        diags: crate::lexer::take_diags(),
    })
}

/// Compile code that runs in a frame it did not seed — an `include`d file or
/// `eval()`'d code, both of which execute in the scope that reached them.
///
/// Its variables are therefore addressed by NAME (no slot numbering, no
/// promotion), its counters continue from `start`, the by-reference signatures
/// of the functions already loaded are known to its call sites, and every
/// chunk it produces is stamped with `source` so the frames and diagnostics of
/// its code name that file.
pub fn compile_nested(stmts: &[Stmt], start: Counters, source: &str) -> Result<Program, String> {
    let mut c = Compiler {
        tmp: start.tmp,
        static_slot: start.static_slot,
        anon_classes: start.anon_classes,
        try_base: start.try_defs,
        ..Compiler::default()
    };
    c.seed_builtin_byref();
    c.seed_loaded_byref();
    c.known_traits = host::with_host(|h| h.trait_defs());
    c.collect_byref(stmts);
    let mut b = ChunkBuilder::new();
    let saved = c.enter_scope(Vec::new());
    c.compile_top_level(&mut b, stmts)?;
    c.resolve_gotos(&mut b, 0, true)?;
    c.leave_scope(saved);
    let counters = c.counters();
    let (fn_sites, class_sites) = c.decl_sites();
    let mut prog = Program {
        fn_sites,
        class_sites,
        main: b.build(),
        main_locals: Vec::new(),
        main_order: Vec::new(),
        main_promoted: Vec::new(),
        functions: c.functions,
        classes: c.classes,
        try_defs: c.try_defs,
        counters,
        diags: crate::lexer::take_diags(),
    };
    stamp_source(&mut prog, source);
    Ok(prog)
}

/// Mark every chunk of `prog` as compiled from `source`.
fn stamp_source(prog: &mut Program, source: &str) {
    fn chunk(c: &mut Chunk, source: &str) {
        c.source = source.to_string();
        for sub in &mut c.sub_chunks {
            chunk(sub, source);
        }
    }
    fn func(f: &mut FuncDef, source: &str) {
        chunk(&mut f.chunk, source);
        for p in &mut f.params {
            if let Some(d) = &mut p.default {
                chunk(d, source);
            }
        }
    }
    chunk(&mut prog.main, source);
    for (_, f) in &mut prog.functions {
        func(f, source);
    }
    for (_, class) in &mut prog.classes {
        for m in class.methods.values_mut() {
            func(m, source);
        }
        for (_, c) in class
            .consts
            .iter_mut()
            .chain(class.prop_defaults.iter_mut())
            .chain(class.static_prop_defaults.iter_mut())
        {
            chunk(c, source);
        }
        for (_, c) in &mut class.enum_cases {
            if let Some(c) = c {
                chunk(c, source);
            }
        }
    }
    for t in &mut prog.try_defs {
        chunk(&mut t.try_chunk, source);
        for c in &mut t.catches {
            chunk(&mut c.chunk, source);
        }
        if let Some(f) = &mut t.finally_chunk {
            chunk(f, source);
        }
    }
}

impl Compiler {
    /// Where this compilation's counters stand.
    fn counters(&self) -> Counters {
        Counters {
            tmp: self.tmp,
            static_slot: self.static_slot,
            anon_classes: self.anon_classes,
            try_defs: self.try_base + self.try_defs.len(),
        }
    }

    /// The by-reference parameters of the user functions ALREADY loaded, which a
    /// run-time compilation calls without having seen their declarations.
    fn seed_loaded_byref(&mut self) {
        for (name, params) in host::with_host(|h| h.user_function_params()) {
            let positions: Vec<usize> = params
                .iter()
                .enumerate()
                .filter(|(_, p)| p.1)
                .map(|(i, _)| i)
                .collect();
            if positions.is_empty() {
                continue;
            }
            self.byref_user_fns.insert(
                name.clone(),
                ByRefFn {
                    spelled: name.clone(),
                    byref: positions
                        .iter()
                        .map(|&i| {
                            let (pname, _, variadic) = &params[i];
                            (
                                i,
                                if *variadic {
                                    String::new()
                                } else {
                                    pname.clone()
                                },
                            )
                        })
                        .collect(),
                    params: params.iter().map(|p| p.0.clone()).collect(),
                    variadic: params.iter().any(|p| p.2),
                },
            );
            self.byref_fns.insert(name, positions);
        }
    }

    fn tmp_name(&mut self, tag: &str) -> String {
        self.tmp += 1;
        format!("@{tag}{}", self.tmp)
    }

    /// Compile a formal parameter list, lowering each default-value expression to
    /// its own chunk (run in the callee frame when the argument is omitted).
    /// Shared by named functions, closures, and methods.
    ///
    /// `owner` is the function as the reference names it in a compile-time
    /// diagnostic — `f()`, `K::m()`, `{closure:FILE:LINE}()`.
    fn compile_params(
        &mut self,
        params: &[Param],
        owner: &str,
    ) -> Result<Vec<host::Param>, String> {
        self.check_params(params)?;
        let required_before = self.check_param_defaults(params, owner);
        let mut out = Vec::with_capacity(params.len());
        for (i, p) in params.iter().enumerate() {
            // An optional parameter ahead of a required one is required too.
            let default = match &p.default {
                Some(_) if i < required_before => None,
                Some(expr) => {
                    let mut db = ChunkBuilder::new();
                    self.in_other_frame(|c| c.compile_expr(&mut db, expr))?;
                    Some(db.build())
                }
                None => None,
            };
            out.push(host::Param {
                name: p.name.clone(),
                line: p.line,
                default,
                variadic: p.variadic,
                by_ref: p.by_ref,
                ty: p.ty.clone().map(|mut t| {
                    if implicitly_nullable(p) {
                        t.parts.push("null".to_string());
                    }
                    t
                }),
            });
        }
        Ok(out)
    }

    /// Seed the write-back map with the standard-library functions whose
    /// signature has a by-reference OUT parameter. The call site treats them
    /// exactly like a user function that declared `&$x`: after the call it
    /// reads `ops::BYREF_OUT` at the position and stores it in the caller's
    /// variable, which is how `preg_match($re, $s, $m)` comes to define `$m`.
    ///
    /// A user function of the same name shadows the builtin, and
    /// [`Compiler::collect_byref`] runs after this and overwrites the entry.
    /// Emit the post-call write-back for the by-reference parameters at
    /// `positions`: read each one's final value out of the returning call and
    /// store it back into the caller's argument, leaving the call's own result on
    /// the stack. This is what makes `f($v)` on `function f(int &$x)` leave `$v`
    /// changed — including by the coercion the parameter's declared type applies,
    /// which happens before the body runs at all.
    ///
    /// `guarded` is for the call sites whose callee is only known at run time — a
    /// method call, a static call, `$f(…)`. They cannot say WHICH positions are
    /// by-reference, so they offer every argument that could be written to and let
    /// each write-back test [`ops::BYREF_LIVE`] first. An unguarded write-back at
    /// such a site would store a null into a variable a by-value call never
    /// touched.
    /// Emit the diagnostic for an argument that cannot supply a by-reference
    /// binding, or nothing at all when it can.
    ///
    /// Called with the argument's value already on the stack, so the reference's
    /// ordering is preserved: the argument is evaluated, then judged, and the
    /// arguments after it are only compiled if the judgement let the call live.
    fn emit_byref_arg_diag(
        &mut self,
        b: &mut ChunkBuilder,
        callee: &str,
        argno: u32,
        param: &str,
        class: ByRefArg,
    ) {
        let kind = match class {
            ByRefArg::Lvalue => return,
            ByRefArg::VarTemp => 0,
            ByRefArg::TmpConst => 1,
        };
        b.emit(Op::LoadInt(kind), 0);
        let c = b.add_constant(Value::str(callee.to_string()));
        b.emit(Op::LoadConst(c), 0);
        b.emit(Op::LoadInt(i64::from(argno)), 0);
        let c = b.add_constant(Value::str(param.to_string()));
        b.emit(Op::LoadConst(c), 0);
        b.emit(Op::CallBuiltin(ops::BYREF_ARG_DIAG, 4), self.cur_line);
        b.emit(Op::Pop, 0);
    }

    /// [`Compiler::emit_byref_arg_diag`] for a method or static call: the
    /// callee is resolved at run time (see [`ops::BYREF_ARG_DIAG_M`]), so only
    /// the verdict on the argument is decided here. `push_recv` pushes the
    /// receiver or class again. Emitted directly after argument `index` is
    /// evaluated, so the arguments after a refused one never run.
    fn emit_byref_arg_diag_m(
        &mut self,
        b: &mut ChunkBuilder,
        push_recv: impl FnOnce(&mut Self, &mut ChunkBuilder) -> Result<(), String>,
        method: &Member,
        index: usize,
        arg: &Expr,
    ) -> Result<(), String> {
        let kind = match byref_arg_class(arg) {
            ByRefArg::Lvalue => return Ok(()),
            ByRefArg::VarTemp => 0,
            ByRefArg::TmpConst => 1,
        };
        push_recv(self, b)?;
        self.emit_member(b, method, 0)?;
        b.emit(Op::LoadInt(index as i64 + 1), 0);
        b.emit(Op::LoadInt(kind), 0);
        b.emit(Op::CallBuiltin(ops::BYREF_ARG_DIAG_M, 4), self.cur_line);
        b.emit(Op::Pop, 0);
        Ok(())
    }

    /// The receiver guard a method call needs when it HAS arguments.
    ///
    /// PHP raises `Call to a member function m() on null` before it evaluates
    /// any argument, so an argument that echoes must not run first. With no
    /// arguments there is nothing to observe, so the guard is skipped and a
    /// zero-argument call — the hot shape — costs nothing.
    fn emit_mcall_recv_check(
        &mut self,
        b: &mut ChunkBuilder,
        name: &Member,
        argc: usize,
    ) -> Result<(), String> {
        if argc == 0 {
            return Ok(());
        }
        self.emit_member(b, name, 0)?;
        b.emit(Op::CallBuiltin(ops::MCALL_RECV_CHECK, 2), self.cur_line);
        Ok(())
    }

    /// Push the name of a `->` member: a literal as a constant, a computed one
    /// (`$o->$n`, `$o->{expr}`) as its value. The access op converts that value
    /// to a string itself, so nothing is coerced here.
    fn emit_member(&mut self, b: &mut ChunkBuilder, m: &Member, line: u32) -> Result<(), String> {
        match m {
            Member::Name(n) => {
                let idx = b.add_constant(Value::str(n.clone()));
                b.emit(Op::LoadConst(idx), line);
            }
            Member::Dyn(e) => self.compile_expr(b, e)?,
        }
        Ok(())
    }

    /// A method call's name, stashed when the receiver check will push it a
    /// second time — which it does only for a call WITH arguments.
    fn method_member(
        &mut self,
        b: &mut ChunkBuilder,
        m: &Member,
        argc: usize,
    ) -> Result<Member, String> {
        if argc == 0 {
            return Ok(m.clone());
        }
        self.stash_member(b, m)
    }

    /// A member about to be pushed more than once — a method call's receiver
    /// check, a compound assignment's read and write. A computed name is
    /// evaluated ONCE, here, into a temporary, and the member returned reads
    /// that temporary, so `$o->{f()} .= "x"` calls `f` a single time. A literal
    /// name comes back as it is.
    fn stash_member(&mut self, b: &mut ChunkBuilder, m: &Member) -> Result<Member, String> {
        match m {
            Member::Name(_) => Ok(m.clone()),
            Member::Dyn(e) => {
                let t = self.tmp_name("mn");
                self.emit_set_var(b, &t, |c, b| c.compile_expr(b, e))?;
                Ok(Member::Dyn(Box::new(Expr::Var(t))))
            }
        }
    }

    /// Reject an undefined callee before the arguments run. Both checks are
    /// net-neutral on the stack — they read the callee the call already pushed
    /// and put it back — so each is emitted directly after that push.
    ///
    /// Emitted only when there IS an argument: with none there is nothing whose
    /// evaluation could be observed ahead of the diagnostic, and the check would
    /// be a second lookup bought for nothing.
    fn emit_callee_check(&mut self, b: &mut ChunkBuilder, op: u16, argc: usize) {
        if argc == 0 {
            return;
        }
        b.emit(Op::CallBuiltin(op, 1), self.cur_line);
    }

    /// [`Compiler::emit_callee_check`] for a call spelled with a literal name.
    ///
    /// A `__`-prefixed name is left unchecked. The engine reaches several of its
    /// own entry points by synthesizing an ordinary call — `(object) $x` lowers
    /// to `__cast_object($x)`, and a `rust { … }` block is rewritten to
    /// `__rust_compile("…", n)` in the SOURCE before it is even lexed — so by
    /// this point neither is distinguishable from something the program wrote.
    /// None of them is a function a PHP program can see, so no name predicate
    /// that is right for `function_exists` can also be right here.
    ///
    /// Skipping is the safe direction: it forgoes an EARLIER diagnostic and
    /// leaves the call to fail exactly as it did before, whereas a wrong refusal
    /// would stop a program that works.
    fn emit_call_name_check(&mut self, b: &mut ChunkBuilder, name: &str, argc: usize) {
        if name.starts_with("__") {
            return;
        }
        self.emit_callee_check(b, ops::CALL_NAME_CHECK, argc);
    }

    /// The callee guard `C::m(…)` needs when it HAS arguments, replacing the
    /// bare class check: `ops::SCALL_CALLEE_CHECK` decides the class, the method
    /// and its reachability in one op, and "class not found" is its first arm.
    ///
    /// Net-neutral on the stack — it consumes the class the call already pushed
    /// plus a copy of the method name, and puts the class back — so it sits
    /// between the class reference and the method name the call itself loads.
    fn emit_scall_callee_check(&mut self, b: &mut ChunkBuilder, name_idx: u16, argc: usize) {
        if argc == 0 {
            return;
        }
        b.emit(Op::LoadConst(name_idx), 0);
        b.emit(Op::CallBuiltin(ops::SCALL_CALLEE_CHECK, 2), self.cur_line);
    }

    fn emit_byref_writeback(
        &mut self,
        b: &mut ChunkBuilder,
        args: &[Expr],
        positions: &[usize],
        guarded: bool,
    ) -> Result<(), String> {
        let targets: Vec<(Expr, usize)> = positions
            .iter()
            .filter_map(|&pos| args.get(pos).map(|a| (a.clone(), pos)))
            .collect();
        self.emit_byref_writeback_to(b, &targets, guarded)
    }

    /// [`Compiler::emit_byref_writeback`] over explicit `(argument, parameter
    /// position)` pairs, for a call whose arguments do not sit at their
    /// parameters' positions — a named argument binds wherever its name is.
    fn emit_byref_writeback_to(
        &mut self,
        b: &mut ChunkBuilder,
        targets: &[(Expr, usize)],
        guarded: bool,
    ) -> Result<(), String> {
        for (arg, pos) in targets {
            let (arg, pos) = (arg, *pos);
            // Only an lvalue can receive one. A literal or a call result in a
            // by-reference position is a diagnostic in the reference, not a write.
            if !matches!(
                arg,
                Expr::Var(_) | Expr::Index(..) | Expr::PropGet(..) | Expr::StaticProp(..)
            ) {
                continue;
            }
            let skip = if guarded {
                b.emit(Op::LoadInt(pos as i64), 0);
                b.emit(Op::CallBuiltin(ops::BYREF_LIVE, 1), 0);
                Some(b.emit(Op::JumpIfFalse(0), 0))
            } else {
                None
            };
            match arg {
                Expr::Var(vname) => {
                    let nidx = b.add_constant(Value::str(vname.clone()));
                    b.emit(Op::LoadConst(nidx), 0);
                    b.emit(Op::LoadInt(pos as i64), 0);
                    b.emit(Op::CallBuiltin(ops::BYREF_OUT, 1), 0);
                    b.emit(Op::CallBuiltin(ops::SETVAR, 2), 0);
                    b.emit(Op::Pop, 0);
                }
                // `f($a[k])` / `f($o->p)` against a by-reference parameter writes
                // back into the element or the property, so the OUT value is parked
                // in a temporary and assigned through the normal lvalue path (which
                // knows how to reach either).
                _ => {
                    let tmp = self.tmp_name("bo");
                    self.emit_set_var(b, &tmp, |_, b| {
                        b.emit(Op::LoadInt(pos as i64), 0);
                        b.emit(Op::CallBuiltin(ops::BYREF_OUT, 1), 0);
                        Ok(())
                    })?;
                    // Straight to `compile_assign`: whether a by-reference argument
                    // may be a temporary is decided where the reference knows the
                    // callee, not by the write-context check of a written `=`.
                    self.compile_assign(b, &arg, None, &Expr::Var(tmp))?;
                    b.emit(Op::Pop, 0);
                }
            }
            if let Some(j) = skip {
                let end = b.current_pos();
                b.patch_jump(j, end);
            }
        }
        Ok(())
    }

    fn seed_builtin_byref(&mut self) {
        const BYREF_BUILTINS: &[(&str, &[usize])] = &[
            ("preg_match", &[2]),
            ("preg_match_all", &[2]),
            ("preg_replace", &[4]),
            ("preg_filter", &[4]),
            ("preg_replace_callback", &[4]),
            ("preg_replace_callback_array", &[3]),
            ("parse_str", &[1]),
            ("similar_text", &[2]),
            ("str_replace", &[3]),
            ("settype", &[0]),
        ];
        for (name, positions) in BYREF_BUILTINS {
            self.byref_fns.insert(name.to_string(), positions.to_vec());
        }
    }

    /// The by-reference OUT positions of the builtins whose by-ref tail is
    /// VARIADIC, as `(name, first by-reference position)`. `sscanf($s, $fmt,
    /// &$a, &$b, …)` takes every argument from index 2 on by reference, so the
    /// position list is a property of the CALL, not of the function.
    const BYREF_VARIADIC_BUILTINS: &'static [(&'static str, usize)] = &[("sscanf", 2)];

    /// The by-reference argument positions of a call to `name` with `nargs`
    /// arguments, plus whether the write-back needs the run-time
    /// [`ops::BYREF_LIVE`] guard.
    ///
    /// A variadic by-ref builtin needs the guard even though its callee is known:
    /// it decides *per call* how many of those positions it actually assigns —
    /// `sscanf` leaves a variable no conversion reached completely untouched,
    /// which an unguarded write-back would overwrite with null.
    fn byref_positions(&self, name: &str, nargs: usize) -> Option<(Vec<usize>, bool)> {
        let lname = name.to_ascii_lowercase();
        if let Some(p) = self.byref_fns.get(&lname) {
            return Some((p.clone(), false));
        }
        Self::BYREF_VARIADIC_BUILTINS
            .iter()
            .find(|(n, _)| *n == lname)
            .map(|&(_, from)| ((from..nargs).collect(), true))
    }

    /// The by-reference positions of a call to the USER function `name` that this
    /// call's `nargs` arguments actually fill, as `(declared spelling, [(0-based
    /// index, 1-based argument number, parameter name)])`.
    ///
    /// An argument in one of these positions has to be a location the callee can
    /// write through; a literal is refused outright and a call's temporary is
    /// allowed with a notice, exactly as for the by-reference BUILTINS (see
    /// [`byref_diag_slots`], whose table this is the user-declared counterpart
    /// of). A VARIADIC by-reference parameter covers every argument from its own
    /// position on, and names none of them.
    ///
    /// Only a call that names its callee is judged here. `$o->m(1)`, `$f(1)` and
    /// `call_user_func('f', 1)` reach a callee the compiler does not know, and the
    /// reference judges those at run time — `call_user_func` does not even raise
    /// the same diagnostic, but a `Warning: … must be passed by reference, value
    /// given`.
    fn byref_user_slots(&self, name: &str, nargs: usize) -> Option<ByRefSlots> {
        let f = self.byref_user_fns.get(&name.to_ascii_lowercase())?;
        let mut slots = Vec::new();
        for (pos, pname) in &f.byref {
            if pname.is_empty() {
                slots.extend((*pos..nargs).map(|i| (i, i as u32 + 1, String::new())));
            } else if *pos < nargs {
                slots.push((*pos, *pos as u32 + 1, pname.clone()));
            }
        }
        Some((f.spelled.clone(), slots))
    }

    /// Pre-pass: record the by-reference parameter positions of every `function`
    /// declaration (recursing into nested bodies) so call sites can write the
    /// callee's finals back to the caller — even for forward references.
    /// Index into `self.loops` of the loop a `break`/`continue` of `level`
    /// targets — level 1 is the innermost — or `None` when the chunk does not
    /// have that many enclosing loops.
    fn loop_at_level(&self, level: u32) -> Option<usize> {
        self.loops.len().checked_sub(level.max(1) as usize)
    }

    /// `zend_compile_break_continue`'s compile-time warning for a `continue`
    /// that lands on a `switch`, which acts as a `break`. When a loop encloses
    /// the switch the warning suggests the level that would reach it.
    fn warn_continue_targets_switch(&self, level: u32, has_parent: bool, line: u32) {
        let mut msg = if level == 1 {
            "\"continue\" targeting switch is equivalent to \"break\"".to_string()
        } else {
            format!("\"continue {level}\" targeting switch is equivalent to \"break {level}\"")
        };
        if has_parent {
            msg.push_str(&format!(
                ". Did you mean to use \"continue {}\"?",
                level + 1
            ));
        }
        crate::lexer::push_diag("Warning", crate::errlevel::E_WARNING, line, msg);
    }

    /// Enter a function, method or closure body: no `try` and no loop around
    /// it is visible from inside. Returns what [`Self::leave_own_loop_scope`]
    /// puts back. (`self.loops` itself is saved by each caller.)
    fn enter_own_loop_scope(&mut self) -> OwnScope {
        self.chunk_seq += 1;
        OwnScope {
            in_try: std::mem::take(&mut self.in_try),
            outer_loops: std::mem::take(&mut self.outer_loops),
            gotos: std::mem::take(&mut self.gotos),
            chunk: std::mem::replace(&mut self.cur_chunk, self.chunk_seq),
        }
    }

    fn leave_own_loop_scope(&mut self, s: OwnScope) {
        self.in_try = s.in_try;
        self.outer_loops = s.outer_loops;
        self.gotos = s.gotos;
        self.cur_chunk = s.chunk;
    }

    /// `label:` — record where it stands.
    fn define_label(&mut self, b: &ChunkBuilder, name: &str, line: u32) -> Result<(), String> {
        if self.gotos.labels.contains_key(name) {
            return Err(self.compile_fatal(line, &format!("Label '{name}' already defined")));
        }
        let at = LabelAt {
            pos: b.current_pos(),
            chunk: self.cur_chunk,
            loops: self.loops.iter().map(|l| l.id).collect(),
        };
        self.gotos.labels.insert(name.to_string(), at);
        Ok(())
    }

    /// Patch every waiting `goto` of chunk `chunk` whose label is known. With
    /// `last`, the function is complete and a `goto` still waiting has no label.
    /// The checks are `zend_resolve_goto_label`'s.
    fn resolve_gotos(
        &mut self,
        b: &mut ChunkBuilder,
        chunk: usize,
        last: bool,
    ) -> Result<(), String> {
        let pending = std::mem::take(&mut self.gotos.pending);
        for g in pending {
            if g.chunk != chunk {
                self.gotos.pending.push(g);
                continue;
            }
            match self.gotos.labels.get(&g.label) {
                Some(at) if at.chunk == chunk => {
                    if !g.loops.starts_with(&at.loops) {
                        return Err(self.compile_fatal(
                            g.line,
                            "'goto' into loop or switch statement is disallowed",
                        ));
                    }
                    b.patch_jump(g.jump, at.pos);
                }
                Some(_) => {
                    return Err(format!(
                        "'goto' across a try/catch/finally boundary is not supported (label '{}')",
                        g.label
                    ))
                }
                None if last => {
                    let msg = format!("'goto' to undefined label '{}'", g.label);
                    return Err(self.compile_fatal(g.line, &msg));
                }
                None => self.gotos.pending.push(g),
            }
        }
        if last {
            if let Some(g) = self.gotos.pending.first() {
                return Err(format!(
                    "'goto' across a try/catch/finally boundary is not supported (label '{}')",
                    g.label
                ));
            }
        }
        Ok(())
    }

    /// How a `break`/`continue` of `level` leaves this chunk: `Ok(Some(i))` jumps
    /// to `self.loops[i]`, `Ok(None)` is a control signal relayed out of a
    /// detached `try` chunk, and `Err` is the compile error
    /// `zend_compile_break_continue` raises — a level of 0, or more levels than
    /// there are enclosing loops in this function.
    ///
    /// A `continue` that lands on a `switch` — here or in an enclosing chunk —
    /// also raises the reference's compile-time warning.
    fn break_target(&self, kw: &str, level: u32, line: u32) -> Result<Option<usize>, String> {
        let depth = self.loops.len() + self.outer_loops.len();
        if level == 0 || level as usize > depth {
            return Err(self.compile_fatal(line, &break_level_error(kw, level, depth)));
        }
        let target = depth - level as usize;
        let is_switch = match target.checked_sub(self.outer_loops.len()) {
            Some(i) => self.loops[i].is_switch,
            None => self.outer_loops[target],
        };
        if kw == "continue" && is_switch {
            self.warn_continue_targets_switch(level, target > 0, line);
        }
        Ok(self.loop_at_level(level))
    }

    fn collect_byref(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            match &s.kind {
                StmtKind::Function {
                    name, params, body, ..
                } => {
                    let positions: Vec<usize> = params
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| p.by_ref)
                        .map(|(i, _)| i)
                        .collect();
                    if !positions.is_empty() {
                        let named = positions
                            .iter()
                            // A VARIADIC by-reference parameter has no name to
                            // print — the reference writes `Argument #2 could
                            // not be passed by reference` with no `($…)` — and
                            // it covers every position from its own on.
                            .map(|&i| {
                                (
                                    i,
                                    if params[i].variadic {
                                        String::new()
                                    } else {
                                        params[i].name.clone()
                                    },
                                )
                            })
                            .collect();
                        self.byref_user_fns.insert(
                            name.to_ascii_lowercase(),
                            ByRefFn {
                                spelled: name.clone(),
                                byref: named,
                                params: params.iter().map(|p| p.name.clone()).collect(),
                                variadic: params.iter().any(|p| p.variadic),
                            },
                        );
                        self.byref_fns.insert(name.to_ascii_lowercase(), positions);
                    }
                    self.collect_byref(body);
                }
                StmtKind::If {
                    then, elifs, els, ..
                } => {
                    self.collect_byref(then);
                    for (_, body) in elifs {
                        self.collect_byref(body);
                    }
                    if let Some(e) = els {
                        self.collect_byref(e);
                    }
                }
                StmtKind::While { body, .. }
                | StmtKind::DoWhile { body, .. }
                | StmtKind::For { body, .. }
                | StmtKind::Foreach { body, .. }
                | StmtKind::Block(body) => self.collect_byref(body),
                StmtKind::Switch { cases, .. } => {
                    for c in cases {
                        self.collect_byref(&c.body);
                    }
                }
                StmtKind::Try {
                    body,
                    catches,
                    finally,
                } => {
                    self.collect_byref(body);
                    for c in catches {
                        self.collect_byref(&c.body);
                    }
                    if let Some(f) = finally {
                        self.collect_byref(f);
                    }
                }
                _ => {}
            }
        }
    }

    /// A file's top level. The reference binds a trait that uses no other trait
    /// EARLY — before any statement runs — which is why a class may `use` a
    /// trait declared further down the file. A trait that uses traits is not
    /// early-bound, even when those were declared above it, and is declared
    /// where it stands, as is everything else.
    fn compile_top_level(&mut self, b: &mut ChunkBuilder, stmts: &[Stmt]) -> Result<(), String> {
        self.early_bind(stmts);
        let mut hoisted: FxHashSet<usize> = FxHashSet::default();
        for (i, s) in stmts.iter().enumerate() {
            if let StmtKind::Class(d) = &s.kind {
                if d.is_trait && d.uses.is_empty() {
                    self.top_decl = true;
                    self.compile_stmt(b, s)?;
                    hoisted.insert(i);
                }
            }
        }
        for (i, s) in stmts.iter().enumerate() {
            if !hoisted.contains(&i) {
                self.top_decl = true;
                self.compile_stmt(b, s)?;
            }
        }
        Ok(())
    }

    /// The reference's EARLY binding: before the file runs, each top-level type
    /// that implements no interface, uses no trait and is not an enum is
    /// entered into the class table in source order — provided its parent (if
    /// any) is already there and its own name is not. Every other top-level
    /// type is declared where it stands, and it is THERE that a clash with a
    /// type bound earlier is reported. That is why, in
    /// `class B extends A {} class A {} class B {}`, the fatal names the SECOND
    /// `B` as the one previously declared: it was bound first.
    ///
    /// Only the order matters to this engine, which loads every type before
    /// the program runs whichever way the reference binds it; what the
    /// simulation decides is which declaration a redeclaration is reported at.
    fn early_bind(&mut self, stmts: &[Stmt]) {
        if self.prelude {
            return;
        }
        for s in stmts {
            match &s.kind {
                StmtKind::Block(body) => self.early_bind(body),
                StmtKind::Class(d)
                    if d.implements.is_empty() && d.uses.is_empty() && !d.is_enum =>
                {
                    let linked = match &d.parent {
                        None => true,
                        Some(p) => {
                            let lp = p.to_ascii_lowercase();
                            self.class_decls.contains_key(&lp)
                                || host::with_host(|h| h.class_exists(&lp))
                                || crate::prelude_type(&lp).is_some()
                        }
                    };
                    if linked && self.class_redeclare_msg(d).is_none() {
                        self.bind_class(d, s.line);
                        self.early.insert(s as *const Stmt as usize);
                    }
                }
                _ => {}
            }
        }
    }

    /// Enter a top-level type into [`Compiler::class_decls`].
    fn bind_class(&mut self, d: &ClassDecl, line: u32) {
        self.class_decls.insert(
            d.name.to_ascii_lowercase(),
            ClassSite {
                kind: decl_kind_word(d),
                name: d.name.clone(),
                line,
                namespace: d.namespace.clone(),
            },
        );
    }

    /// The reference's `Cannot redeclare <kind> <Name> …` for declaring `d`
    /// at the top level now, or `None` when its name is free. It names the
    /// type ALREADY declared, which is not necessarily spelled like `d`.
    fn class_redeclare_msg(&self, d: &ClassDecl) -> Option<String> {
        if self.prelude {
            return None;
        }
        match self.class_decls.get(&d.name.to_ascii_lowercase()) {
            Some(old) if old.namespace == d.namespace => {
                let file = host::with_host(|h| h.script_name().to_string());
                Some(format!(
                    "Cannot redeclare {} {} (previously declared in {file}:{})",
                    old.kind, old.name, old.line
                ))
            }
            Some(_) => None,
            None => host::with_host(|h| h.class_redeclare_msg(&d.name, &d.namespace)),
        }
    }

    /// The reference's `Cannot redeclare function …` for declaring `name` at
    /// the top level, or `None` when it is free. The reference raises it while
    /// COMPILING — the file prints nothing — because a top-level function is
    /// entered into the function table before the file runs.
    fn fn_redeclare_msg(&self, name: &str, ns: &str) -> Option<String> {
        if self.prelude {
            return None;
        }
        match self.fn_decls.get(&name.to_ascii_lowercase()) {
            Some((line, ns0)) if ns0 == ns => {
                let file = host::with_host(|h| h.script_name().to_string());
                Some(format!(
                    "Cannot redeclare function {name}() (previously declared in {file}:{line})"
                ))
            }
            Some(_) => None,
            None => host::with_host(|h| h.fn_redeclare_msg(name, ns)),
        }
    }

    /// A compile-time `Fatal error` at `line`, marked with [`COMPILE_FATAL`].
    fn compile_fatal(&self, line: u32, msg: &str) -> String {
        let file = host::with_host(|h| h.script_name().to_string());
        format!("{COMPILE_FATAL}{msg} in {file} on line {line}\nStack trace:\n#0 {{main}}")
    }

    /// The declaration sites this compilation's top level recorded, for
    /// [`Program::fn_sites`] and [`Program::class_sites`].
    #[allow(clippy::type_complexity)]
    fn decl_sites(&self) -> (Vec<(String, u32, String)>, Vec<(String, u32, String)>) {
        let fns = self
            .fn_decls
            .iter()
            .map(|(n, (line, ns))| (n.clone(), *line, ns.clone()))
            .collect();
        let classes = self
            .class_decls
            .iter()
            .map(|(n, s)| (n.clone(), s.line, s.namespace.clone()))
            .collect();
        (fns, classes)
    }

    /// Lower a declaration that is NOT at the top level: its definition was
    /// just pushed under its own name as the last entry of `table`; move it to
    /// a key no PHP name can spell and emit the `op` that binds it when the
    /// statement runs.
    fn defer_declaration<T>(
        &mut self,
        b: &mut ChunkBuilder,
        table: fn(&mut Self) -> &mut Vec<(String, T)>,
        op: u16,
        operands: &[String],
        line: u32,
    ) {
        let key = self.tmp_name("decl");
        if let Some(last) = table(self).last_mut() {
            last.0 = key.clone();
        }
        for v in std::iter::once(&key).chain(operands) {
            let idx = b.add_constant(Value::str(v.clone()));
            b.emit(Op::LoadConst(idx), line);
        }
        b.emit(Op::CallBuiltin(op, 1 + operands.len() as u8), line);
        b.emit(Op::Pop, line);
    }

    fn compile_seq(&mut self, b: &mut ChunkBuilder, body: &[Stmt]) -> Result<(), String> {
        for s in body {
            self.compile_stmt(b, s)?;
        }
        Ok(())
    }

    fn compile_stmt(&mut self, b: &mut ChunkBuilder, s: &Stmt) -> Result<(), String> {
        let top = std::mem::take(&mut self.top_decl);
        // Under `--dap` each statement is preceded by a `DBG_LINE` marker so the
        // debugger can stop on it; the builtin returns Undef, popped immediately.
        if self.debug && s.line != 0 {
            b.emit(Op::LoadInt(s.line as i64), s.line);
            b.emit(Op::CallBuiltin(ops::DBG_LINE, 1), s.line);
            b.emit(Op::Pop, s.line);
        }
        let line = s.line;
        if line != 0 {
            self.cur_line = line;
        }
        match &s.kind {
            StmtKind::InlineHtml(text) => {
                let idx = b.add_constant(Value::str(text.clone()));
                b.emit(Op::LoadConst(idx), line);
                b.emit(Op::CallBuiltin(ops::ECHO, 1), line);
                b.emit(Op::Pop, line);
            }
            StmtKind::Echo(args) => {
                // `echo a, b, c` emits each argument as it is evaluated (PHP
                // outputs left to right), so a side effect inside a later argument
                // (e.g. a generator method that echoes) interleaves correctly.
                for a in args {
                    self.compile_expr(b, a)?;
                    b.emit(Op::CallBuiltin(ops::ECHO, 1), line);
                    b.emit(Op::Pop, line);
                }
            }
            // A bare `$name;` reads a compiled variable whose value nothing
            // wants, which the reference compiles to no opcode at all — so an
            // undefined one raises no `Undefined variable` warning.
            StmtKind::Expr(Expr::Var(name))
                if name != "this" && !crate::host::is_superglobal(name) => {}
            StmtKind::Expr(e) => {
                self.compile_expr(b, e)?;
                b.emit(Op::Pop, line);
            }
            // A plain `{ }` or `namespace { }` block at the top level keeps its
            // statements at the top level.
            StmtKind::Block(body) if top => {
                for s in body {
                    self.top_decl = true;
                    self.compile_stmt(b, s)?;
                }
            }
            StmtKind::Block(body) => self.compile_seq(b, body)?,
            StmtKind::Global(names) => {
                for name in names {
                    if name == "this" {
                        return Err(self.compile_fatal(line, "Cannot use $this as global variable"));
                    }
                    let nidx = b.add_constant(Value::str(name.clone()));
                    b.emit(Op::LoadConst(nidx), line);
                    b.emit(Op::CallBuiltin(ops::GLOBAL_BIND, 1), line);
                    b.emit(Op::Pop, 0);
                }
            }
            StmtKind::StaticLocal(decls) => {
                for (name, default) in decls {
                    if name == "this" {
                        return Err(self.compile_fatal(line, "Cannot use $this as static variable"));
                    }
                    // A unique, stable key per declaration — baked into the chunk
                    // so every call resolves the same persistent slot.
                    let key = format!("@static#{}", self.static_slot);
                    self.static_slot += 1;
                    let nidx = b.add_constant(Value::str(name.clone()));
                    b.emit(Op::LoadConst(nidx), line);
                    let kidx = b.add_constant(Value::str(key));
                    b.emit(Op::LoadConst(kidx), line);
                    match default {
                        Some(e) => self.compile_expr(b, e)?,
                        None => {
                            b.emit(Op::LoadUndef, line);
                        }
                    }
                    b.emit(Op::CallBuiltin(ops::STATIC_BIND, 3), line);
                    b.emit(Op::Pop, line);
                }
            }
            StmtKind::ConstDecl(decls) => {
                // In source order, and each value evaluated where it stands: a
                // later entry may READ an earlier one (`const A = 1, B = A + 1;`),
                // which only works if the earlier write has already happened.
                for (name, value) in decls {
                    let nidx = b.add_constant(Value::str(name.clone()));
                    b.emit(Op::LoadConst(nidx), line);
                    self.compile_expr(b, value)?;
                    b.emit(Op::CallBuiltin(ops::CONST_DECL, 2), line);
                    b.emit(Op::Pop, line);
                }
            }
            StmtKind::Return(e) => {
                if let Some(msg) = self.return_refusal(e.as_ref()) {
                    return Err(self.compile_fatal(self.cur_line, &msg));
                }
                // Inside a `function &f()`, a `return` naming an lvalue publishes
                // that storage cell (and still leaves its value, which is what a
                // plain call sees). A returned expression that is not an lvalue
                // has no cell to publish and takes the by-value path.
                let by_ref = self.ret_by_ref
                    && matches!(
                        e,
                        Some(Expr::Var(_)) | Some(Expr::Index(..)) | Some(Expr::PropGet(..))
                    );
                match e {
                    Some(e) if by_ref => {
                        self.compile_ref_slot(b, e)?;
                        b.emit(Op::CallBuiltin(ops::RET_REF, 1), line);
                    }
                    Some(e) => self.compile_expr(b, e)?,
                    None => {
                        b.emit(Op::LoadUndef, line);
                    }
                }
                // The value is computed first; then every generator loop this
                // `return` leaves frees its iterator, as the reference does
                // before the frame exits.
                let subjects: Vec<(String, String)> = self
                    .loops
                    .iter()
                    .rev()
                    .filter_map(|l| l.gen_subj.clone())
                    .collect();
                for (s, m) in &subjects {
                    self.emit_gen_release(b, s, m)?;
                }
                b.emit(Op::CallBuiltin(ops::SIG_RETURN, 1), line);
                b.emit(Op::Pop, line);
            }
            StmtKind::Label(name) => self.define_label(b, name, line)?,
            StmtKind::Goto(name) => {
                let jump = b.emit(Op::Jump(0), line);
                let g = PendingGoto {
                    label: name.clone(),
                    jump,
                    chunk: self.cur_chunk,
                    loops: self.loops.iter().map(|l| l.id).collect(),
                    line,
                };
                self.gotos.pending.push(g);
            }
            StmtKind::Break(level) => {
                // `break n` leaves the n-th enclosing loop, so index the loop
                // stack from the top: level 1 is the innermost. Inside a loop in
                // this chunk → an in-chunk jump. Inside a `try` body with no such
                // loop → a control signal the orchestrator relays to the
                // enclosing loop.
                if let Some(idx) = self.break_target("break", *level, line)? {
                    let j = b.emit(Op::Jump(0), line);
                    self.loops[idx].breaks.push(j);
                } else {
                    // No loop for it in this chunk: raise a signal carrying the
                    // levels still to unwind, which the `try` dispatch in the
                    // enclosing chunk resolves (or re-raises, decremented).
                    b.emit(Op::LoadInt(*level as i64), line);
                    b.emit(Op::CallBuiltin(ops::SIG_BREAK, 1), line);
                    b.emit(Op::Pop, line);
                }
            }
            StmtKind::Continue(level) => {
                if let Some(idx) = self.break_target("continue", *level, line)? {
                    let j = b.emit(Op::Jump(0), line);
                    self.loops[idx].continues.push(j);
                } else {
                    b.emit(Op::LoadInt(*level as i64), line);
                    b.emit(Op::CallBuiltin(ops::SIG_CONTINUE, 1), line);
                    b.emit(Op::Pop, line);
                }
            }
            StmtKind::Try {
                body,
                catches,
                finally,
            } => self.compile_try(b, body, catches, finally.as_deref(), line)?,
            StmtKind::Function {
                name,
                params,
                body,
                ret,
                by_ref_return,
                namespace,
                deprecated,
            } => {
                if top {
                    if let Some(msg) = self.fn_redeclare_msg(name, namespace) {
                        return Err(self.compile_fatal(line, &msg));
                    }
                    self.fn_decls
                        .insert(name.to_ascii_lowercase(), (line, namespace.clone()));
                }
                // Each default-value expression is lowered to its own tiny chunk,
                // run in the callee frame when the argument is omitted (host).
                let owner = if namespace.is_empty() {
                    format!("{name}()")
                } else {
                    format!("{namespace}\\{name}()")
                };
                let cparams = self.compile_params(params, &owner)?;
                let mut fb = ChunkBuilder::new();
                // A function body has its own loop scope: a break inside it must
                // not target a loop at the call site.
                let saved = std::mem::take(&mut self.loops);
                let saved_try = self.enter_own_loop_scope();
                let saved_ref = std::mem::replace(&mut self.ret_by_ref, *by_ref_return);
                let saved_rule =
                    std::mem::replace(&mut self.ret_rule, ret_rule(ret.as_ref(), body));
                // A closure written in this body is `{closure:name():LINE}`. PHP
                // spells the enclosing function with its parentheses and in its
                // DECLARED casing, not the lowercased lookup key.
                let saved_site = std::mem::replace(
                    &mut self.decl_site,
                    host::DeclSite::Named(format!("{name}()")),
                );
                // The body addresses its own frame, so its variables get slots.
                // A parameter default is NOT compiled here — it runs as its own
                // chunk and keeps the by-name path, which reaches the same slots.
                let promoted = self.promotable_locals(params, body, body_has_yield(body));
                let locals = self.enter_scope_promoting(scope_slots(params, body), promoted);
                self.compile_seq(&mut fb, body)?;
                self.resolve_gotos(&mut fb, self.cur_chunk, true)?;
                let locals = self.leave_scope(locals);
                self.decl_site = saved_site;
                self.ret_by_ref = saved_ref;
                self.loops = saved;
                self.leave_own_loop_scope(saved_try);
                self.ret_rule = saved_rule;
                self.functions.push((
                    name.to_ascii_lowercase(),
                    FuncDef {
                        params: cparams,
                        chunk: fb.build(),
                        is_generator: body_has_yield(body),
                        ret: ret.clone(),
                        locals,
                        // A named function's frame is named by the function.
                        closure_site: None,
                        declared: Some(if namespace.is_empty() {
                            name.clone()
                        } else {
                            format!("{namespace}\\{name}")
                        }),
                        deprecated: deprecated.clone(),
                    },
                ));
                if !top {
                    self.defer_declaration(
                        b,
                        |c| &mut c.functions,
                        ops::DECLARE_FN,
                        &[name.clone(), namespace.clone()],
                        line,
                    );
                }
            }
            StmtKind::Class(decl) if top => {
                let early = self.early.contains(&(s as *const Stmt as usize));
                if !early {
                    // Declared where it stands, so a clash with a type bound
                    // before it is reported here, after whatever ran first.
                    if let Some(msg) = self.class_redeclare_msg(decl) {
                        let idx = b.add_constant(Value::str(msg));
                        b.emit(Op::LoadConst(idx), line);
                        b.emit(Op::CallBuiltin(ops::DECL_FATAL, 1), line);
                        b.emit(Op::Pop, line);
                        return Ok(());
                    }
                    if !self.prelude {
                        self.bind_class(decl, line);
                    }
                }
                let before = self.classes.len();
                self.compile_class(b, decl)?;
                if self.classes.len() > before {
                    self.check_finals(b, decl, early, line)?;
                }
            }
            StmtKind::Class(decl) => {
                let before = self.classes.len();
                self.compile_class(b, decl)?;
                // Nothing was registered when the declaration cannot link; the
                // fatal that says so is already in the stream.
                if self.classes.len() > before {
                    self.defer_declaration(
                        b,
                        |c| &mut c.classes,
                        ops::DECLARE_CLASS,
                        std::slice::from_ref(&decl.namespace),
                        line,
                    );
                    self.check_finals(b, decl, false, line)?;
                }
            }
            StmtKind::If {
                cond,
                then,
                elifs,
                els,
            } => self.compile_if(b, cond, then, elifs, els)?,
            StmtKind::While { cond, body } => self.compile_while(b, cond, body)?,
            StmtKind::DoWhile { cond, body } => self.compile_do_while(b, cond, body)?,
            StmtKind::Switch { subj, cases } => self.compile_switch(b, subj, cases)?,
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => self.compile_for(b, init, cond.as_ref(), step, body)?,
            StmtKind::Foreach {
                arr,
                key_var,
                val,
                by_ref,
                body,
            } => self.compile_foreach(b, arr, key_var.as_deref(), val, *by_ref, body)?,
        }
        Ok(())
    }

    fn compile_if(
        &mut self,
        b: &mut ChunkBuilder,
        cond: &Expr,
        then: &[Stmt],
        elifs: &[(Expr, Vec<Stmt>)],
        els: &Option<Vec<Stmt>>,
    ) -> Result<(), String> {
        // Flatten if/elseif/else into a chain; each arm jumps to the shared end.
        let mut ends: Vec<usize> = Vec::new();
        let arms: Vec<(&Expr, &[Stmt])> = std::iter::once((cond, then))
            .chain(elifs.iter().map(|(c, body)| (c, body.as_slice())))
            .collect();

        for (c, body) in arms {
            self.compile_truthy(b, c)?;
            let next = b.emit(Op::JumpIfFalse(0), 0);
            self.compile_seq(b, body)?;
            ends.push(b.emit(Op::Jump(0), 0));
            let here = b.current_pos();
            b.patch_jump(next, here);
        }
        if let Some(body) = els {
            self.compile_seq(b, body)?;
        }
        let end = b.current_pos();
        for j in ends {
            b.patch_jump(j, end);
        }
        Ok(())
    }

    fn compile_while(
        &mut self,
        b: &mut ChunkBuilder,
        cond: &Expr,
        body: &[Stmt],
    ) -> Result<(), String> {
        // Rotated: the test is emitted once as an entry guard and once at the
        // BOTTOM, where it becomes a conditional backward branch. fusevm's
        // tracing JIT closes a trace only on such a branch, so a top-test loop
        // ending in an unconditional `Jump` is recorded and then declined —
        // which is what kept every `while` in the interpreter.
        //
        // The test still runs n + 1 times for n iterations, in the same place
        // in the evaluation order; rotation costs one copy of the condition's
        // code and saves one jump per iteration.
        let enter = b.emit(Op::Jump(0), 0);
        let body_pos = b.current_pos();
        self.loop_seq += 1;
        self.loops.push(LoopCtx {
            id: self.loop_seq,
            breaks: vec![],
            continues: vec![],
            gen_subj: None,
            is_switch: false,
        });
        self.compile_seq(b, body)?;
        let ctx = self.loops.pop().unwrap();
        let cond_pos = b.current_pos();
        b.patch_jump(enter, cond_pos);
        self.compile_truthy(b, cond)?;
        b.emit(Op::JumpIfTrue(body_pos), 0);
        let end = b.current_pos();
        for j in ctx.breaks {
            b.patch_jump(j, end);
        }
        for j in ctx.continues {
            b.patch_jump(j, cond_pos);
        }
        Ok(())
    }

    fn compile_do_while(
        &mut self,
        b: &mut ChunkBuilder,
        cond: &Expr,
        body: &[Stmt],
    ) -> Result<(), String> {
        // The body runs once before the condition is ever tested.
        let top = b.current_pos();
        self.loop_seq += 1;
        self.loops.push(LoopCtx {
            id: self.loop_seq,
            breaks: vec![],
            continues: vec![],
            gen_subj: None,
            is_switch: false,
        });
        self.compile_seq(b, body)?;
        let ctx = self.loops.pop().unwrap();
        // `continue` re-tests the condition; `break` exits.
        let cond_pos = b.current_pos();
        self.compile_truthy(b, cond)?;
        b.emit(Op::JumpIfTrue(top), 0);
        let end = b.current_pos();
        for j in ctx.breaks {
            b.patch_jump(j, end);
        }
        for j in ctx.continues {
            b.patch_jump(j, cond_pos);
        }
        Ok(())
    }

    /// `switch`: evaluate the subject once, dispatch to the first `case` whose
    /// value is loosely (`==`) equal (or `default`), then run bodies in source
    /// order so fall-through is natural. `break` exits the switch.
    fn compile_switch(
        &mut self,
        b: &mut ChunkBuilder,
        subj: &Expr,
        cases: &[SwitchCase],
    ) -> Result<(), String> {
        let sw_t = self.tmp_name("sw");
        self.emit_set_var(b, &sw_t, |c, b| c.compile_expr(b, subj))?;

        // Dispatch chain: `@sw == case_value` for each non-default case.
        let mut dispatch: Vec<(usize, usize)> = Vec::new(); // (case index, JumpIfTrue pos)
        let mut default_index: Option<usize> = None;
        for (i, case) in cases.iter().enumerate() {
            match &case.test {
                Some(test) => {
                    self.emit_get_var(b, &sw_t);
                    self.compile_expr(b, test)?;
                    b.emit(Op::CallBuiltin(ops::LOOSE_EQ, 2), 0);
                    b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0);
                    let jt = b.emit(Op::JumpIfTrue(0), 0);
                    dispatch.push((i, jt));
                }
                None => default_index = Some(i),
            }
        }
        // No case matched: fall to `default` if present, else past the switch.
        let fallthrough = b.emit(Op::Jump(0), 0);

        // Bodies, emitted in source order (no jumps between them → fall-through).
        self.loop_seq += 1;
        self.loops.push(LoopCtx {
            id: self.loop_seq,
            breaks: vec![],
            continues: vec![],
            gen_subj: None,
            is_switch: true,
        });
        let mut body_starts = Vec::with_capacity(cases.len());
        for case in cases {
            body_starts.push(b.current_pos());
            self.compile_seq(b, &case.body)?;
        }
        let ctx = self.loops.pop().unwrap();
        let end = b.current_pos();

        for (i, jt) in dispatch {
            b.patch_jump(jt, body_starts[i]);
        }
        match default_index {
            Some(di) => b.patch_jump(fallthrough, body_starts[di]),
            None => b.patch_jump(fallthrough, end),
        }
        for j in ctx.breaks {
            b.patch_jump(j, end);
        }
        // `continue` inside a switch acts like `break` of the switch (PHP treats
        // the switch as a loop level; `continue 1` exits it).
        for j in ctx.continues {
            b.patch_jump(j, end);
        }
        Ok(())
    }

    fn compile_for(
        &mut self,
        b: &mut ChunkBuilder,
        init: &[Expr],
        cond: Option<&Expr>,
        step: &[Expr],
        body: &[Stmt],
    ) -> Result<(), String> {
        for e in init {
            self.compile_expr(b, e)?;
            b.emit(Op::Pop, 0);
        }
        // Rotated, as `while` is: the condition is emitted at the BOTTOM so the
        // back edge is conditional and the loop can be traced. `for (;;)` has no
        // condition to branch on and keeps its unconditional edge.
        let enter = cond.map(|_| b.emit(Op::Jump(0), 0));
        let body_pos = b.current_pos();
        self.loop_seq += 1;
        self.loops.push(LoopCtx {
            id: self.loop_seq,
            breaks: vec![],
            continues: vec![],
            gen_subj: None,
            is_switch: false,
        });
        self.compile_seq(b, body)?;
        let ctx = self.loops.pop().unwrap();
        // `continue` in a for-loop jumps to the step, not the condition.
        let step_pos = b.current_pos();
        for e in step {
            self.compile_expr(b, e)?;
            b.emit(Op::Pop, 0);
        }
        match cond {
            Some(c) => {
                let cond_pos = b.current_pos();
                if let Some(enter) = enter {
                    b.patch_jump(enter, cond_pos);
                }
                self.compile_truthy(b, c)?;
                b.emit(Op::JumpIfTrue(body_pos), 0);
            }
            None => {
                b.emit(Op::Jump(body_pos), 0);
            }
        }
        let end = b.current_pos();
        for j in ctx.breaks {
            b.patch_jump(j, end);
        }
        for j in ctx.continues {
            b.patch_jump(j, step_pos);
        }
        Ok(())
    }

    /// `foreach ($subject as [$k =>] $v) { body }`. A `Generator` subject is driven
    /// lazily through the `Generator` protocol (so side effects interleave and
    /// infinite generators work); everything else desugars to iterating a
    /// materialized key list by index.
    fn compile_foreach(
        &mut self,
        b: &mut ChunkBuilder,
        arr: &Expr,
        key_var: Option<&str>,
        val: &ForeachVal,
        by_ref: bool,
        body: &[Stmt],
    ) -> Result<(), String> {
        // A destructuring target binds through a hidden temporary: the element
        // is bound to it exactly as a plain `$v` would be, and the pattern is
        // then assigned FROM it at the head of the body. Reusing the standalone
        // `[$x, $y] = …` path is what makes a too-short element warn
        // ("Undefined array key N") and then bind null, rather than binding null
        // in silence.
        if key_var == Some("this") || matches!(val, ForeachVal::Var(n) if n == "this") {
            return Err(self.compile_fatal(self.cur_line, "Cannot re-assign $this"));
        }
        let (val_name, pattern) = match val {
            ForeachVal::Var(n) => (n.clone(), None),
            ForeachVal::Pattern(p) => (self.tmp_name("fev"), Some(p)),
        };
        let val_var: &str = &val_name;

        // Evaluate the subject once into a hidden temporary, then branch: a lazy
        // generator loop, or the array/iterator index loop.
        // A subject that is not a plain variable may CREATE the generator it
        // yields; the mark taken first is how the release at the loop's end
        // tells such a fresh generator from one that already existed.
        let mark_t = match arr {
            Expr::Var(_) => None,
            _ => {
                let m = self.tmp_name("gmark");
                self.emit_set_var(b, &m, |_, b| {
                    b.emit(Op::CallBuiltin(ops::GEN_MARK, 0), 0);
                    Ok(())
                })?;
                Some(m)
            }
        };
        let subj_t = self.tmp_name("subj");
        self.emit_set_var(b, &subj_t, |c, b| c.compile_expr(b, arr))?;

        // A by-value `foreach` walks a generator or an `Iterator` one step at
        // a time, interleaved with the body; an `IteratorAggregate` hands over
        // its iterator first. A by-reference one keeps the array walk.
        if !by_ref {
            let line = self.cur_line;
            self.emit_set_var(b, &subj_t, |c, b| {
                c.emit_get_var(b, &subj_t);
                b.emit(Op::CallBuiltin(ops::FOREACH_ITER, 1), line);
                Ok(())
            })?;
        }
        self.emit_get_var(b, &subj_t);
        let lazy = if by_ref {
            ops::IS_GENERATOR
        } else {
            ops::IS_LAZY_ITER
        };
        b.emit(Op::CallBuiltin(lazy, 1), 0);
        b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0);
        let to_array = b.emit(Op::JumpIfFalse(0), 0);
        // The body is lowered twice; its compile-time warnings are kept from the
        // array copy below only, so each is raised once.
        let diags = crate::lexer::diag_count();
        // Labels in the body are defined again by the array copy: those this
        // copy defines serve its own jumps and are then forgotten.
        let labels_before: FxHashSet<String> = self.gotos.labels.keys().cloned().collect();
        self.compile_foreach_generator(
            b,
            &subj_t,
            mark_t.as_deref(),
            key_var,
            val_var,
            pattern,
            body,
        )?;
        crate::lexer::truncate_diags(diags);
        let fresh: Vec<String> = self
            .gotos
            .labels
            .keys()
            .filter(|k| !labels_before.contains(*k))
            .cloned()
            .collect();
        let pending = std::mem::take(&mut self.gotos.pending);
        for g in pending {
            match self.gotos.labels.get(&g.label) {
                Some(at)
                    if fresh.contains(&g.label)
                        && at.chunk == g.chunk
                        && g.chunk == self.cur_chunk =>
                {
                    b.patch_jump(g.jump, at.pos);
                }
                _ => self.gotos.pending.push(g),
            }
        }
        for k in fresh {
            self.gotos.labels.remove(&k);
        }
        let after_gen = b.emit(Op::Jump(0), 0);
        let array_start = b.current_pos();
        b.patch_jump(to_array, array_start);

        let arr_t = self.tmp_name("arr");
        let keys_t = self.tmp_name("keys");
        let i_t = self.tmp_name("i");

        // @arr = foreach_prep(@subj);  @keys = array_keys(@arr);  @i = 0;
        // FOREACH_PREP passes arrays through and materializes an iterable object
        // (Iterator / IteratorAggregate / public properties) into an array.
        self.emit_set_var(b, &arr_t, |c, b| {
            c.emit_get_var(b, &subj_t);
            b.emit(Op::CallBuiltin(ops::FOREACH_PREP, 1), 0);
            Ok(())
        })?;
        self.emit_set_var(b, &keys_t, |c, b| {
            c.emit_get_var(b, &arr_t);
            b.emit(Op::CallBuiltin(ops::ARRAYKEYS, 1), 0);
            Ok(())
        })?;
        self.emit_set_var(b, &i_t, |_, b| {
            b.emit(Op::LoadInt(0), 0);
            Ok(())
        })?;

        let top = b.current_pos();
        // while (@i < count(@keys))
        self.emit_get_var(b, &i_t);
        self.emit_get_var(b, &keys_t);
        b.emit(Op::CallBuiltin(ops::ARRAYLEN, 1), 0);
        b.emit(Op::CallBuiltin(ops::LT, 2), 0);
        b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0);
        let exit = b.emit(Op::JumpIfFalse(0), 0);

        // @k = @keys[@i];  bind key var if present.
        let k_t = self.tmp_name("k");
        self.emit_set_var(b, &k_t, |c, b| {
            c.emit_get_var(b, &keys_t);
            c.emit_get_var(b, &i_t);
            b.emit(Op::CallBuiltin(ops::INDEX_GET_Q, 2), 0);
            Ok(())
        })?;
        // A by-reference `foreach` walks the LIVE array, so a key the body has
        // already unset is skipped. The key list is materialized up front, so
        // that key is still in it; without this guard the `&` binding below
        // auto-vivified it and the element came back as a null.
        //
        // A by-VALUE foreach iterates a snapshot and is unaffected by the same
        // `unset()`, which is why the guard is only on this arm.
        let mut skip_missing = None;
        if by_ref {
            let fi = b.add_constant(Value::str("array_key_exists"));
            b.emit(Op::LoadConst(fi), 0);
            self.emit_get_var(b, &k_t);
            self.emit_get_var(b, &arr_t);
            b.emit(Op::CallBuiltin(ops::CALL, 3), self.cur_line);
            b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0);
            skip_missing = Some(b.emit(Op::JumpIfFalse(0), 0));
        }
        if let Some(kv) = key_var {
            self.emit_set_var(b, kv, |c, b| {
                c.emit_get_var(b, &k_t);
                Ok(())
            })?;
        }
        // A by-value `foreach` binds a *copy* of each element, so writing
        // through `$v` cannot reach the array.
        //
        // A by-reference one binds the ELEMENT — `$v = &@arr[@k]`, the same
        // lowering `$v = &$a[$k]` gets. Copying the element and writing it back
        // after the body (what this used to do) is observably different three
        // ways, all of which the reference gets from the aliasing: a write to
        // `$v` is visible in the array *within* the same iteration; `unset()`ing
        // a later key inside the body does not get it resurrected by a
        // write-back; and after the loop `$v` is still an alias of the LAST
        // element, so `foreach ($a as &$v) {} foreach ($a as $v) {}` leaves
        // `$a`'s tail duplicated exactly as PHP's most-cited gotcha does.
        if by_ref {
            let elem = Expr::Index(
                Box::new(Expr::Var(arr_t.clone())),
                Box::new(Expr::Var(k_t.clone())),
            );
            self.compile_ref_assign(b, &Expr::Var(val_name.clone()), &elem)?;
            b.emit(Op::Pop, 0);
        } else {
            self.emit_set_var(b, val_var, |c, b| {
                c.emit_get_var(b, &arr_t);
                c.emit_get_var(b, &k_t);
                b.emit(Op::CallBuiltin(ops::INDEX_GET_Q, 2), 0);
                b.emit(Op::CallBuiltin(ops::COPY, 1), 0);
                Ok(())
            })?;
        }

        // `@arr` shares the subject array's handle — the by-reference write-back
        // further down relies on the same fact — so `@arr[@k]` is the real
        // element, and it is what a `&` target in the pattern aliases.
        let row_path = Expr::Index(
            Box::new(Expr::Var(arr_t.clone())),
            Box::new(Expr::Var(k_t.clone())),
        );
        self.emit_foreach_destructure(b, pattern, val_var, Some(&row_path))?;

        self.loop_seq += 1;
        self.loops.push(LoopCtx {
            id: self.loop_seq,
            breaks: vec![],
            continues: vec![],
            gen_subj: None,
            is_switch: false,
        });
        self.compile_seq(b, body)?;
        let ctx = self.loops.pop().unwrap();

        // `continue` lands here. A by-reference foreach needs no write-back:
        // `$v` IS the element (see the binding above), so the body already
        // wrote through it.
        let cont_target = b.current_pos();
        if let Some(j) = skip_missing {
            b.patch_jump(j, cont_target);
        }

        // @i = @i + 1;
        self.emit_set_var(b, &i_t, |c, b| {
            c.emit_get_var(b, &i_t);
            b.emit(Op::LoadInt(1), 0);
            b.emit(Op::Add, 0);
            Ok(())
        })?;
        b.emit(Op::Jump(top), 0);
        let end = b.current_pos();
        b.patch_jump(exit, end);
        for j in ctx.breaks {
            b.patch_jump(j, end);
        }
        for j in ctx.continues {
            b.patch_jump(j, cont_target);
        }
        // The generator path jumps here, past the array path.
        b.patch_jump(after_gen, end);
        Ok(())
    }

    /// The lazy `foreach` loop for a `Generator` subject held in `@subj`:
    /// `rewind`, then repeatedly `valid`/`key`/`current`/(body)/`next`. Preserves
    /// side-effect ordering and supports infinite generators (unlike materializing).
    #[allow(clippy::too_many_arguments)]
    fn compile_foreach_generator(
        &mut self,
        b: &mut ChunkBuilder,
        subj_t: &str,
        mark_t: Option<&str>,
        key_var: Option<&str>,
        val_var: &str,
        pattern: Option<&Expr>,
        body: &[Stmt],
    ) -> Result<(), String> {
        // Every step is the `foreach` line's, as the reference's `FE_RESET` /
        // `FE_FETCH` are: a frame the iterator's methods open names it.
        let line = self.cur_line;
        // @subj->rewind();  (prime to the first yield)
        self.emit_get_var(b, subj_t);
        b.emit(Op::CallBuiltin(ops::GEN_REWIND, 1), line);
        b.emit(Op::Pop, 0);

        let top = b.current_pos();
        // while (@subj->valid())
        self.emit_get_var(b, subj_t);
        b.emit(Op::CallBuiltin(ops::GEN_VALID, 1), line);
        b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0);
        let exit = b.emit(Op::JumpIfFalse(0), 0);

        // Bind the value, then the key: `zend_fe_fetch_object_helper` asks the
        // iterator for its current value before its key, and assigns them in
        // that order.
        self.emit_set_var(b, val_var, |c, b| {
            c.emit_get_var(b, subj_t);
            b.emit(Op::CallBuiltin(ops::GEN_CURRENT, 1), line);
            Ok(())
        })?;
        if let Some(kv) = key_var {
            self.emit_set_var(b, kv, |c, b| {
                c.emit_get_var(b, subj_t);
                b.emit(Op::CallBuiltin(ops::GEN_KEY, 1), line);
                Ok(())
            })?;
        }
        // A yielded value is a temporary with no element behind it, so a `&`
        // target in the pattern has nothing to alias.
        self.emit_foreach_destructure(b, pattern, val_var, None)?;

        self.loop_seq += 1;
        self.loops.push(LoopCtx {
            id: self.loop_seq,
            breaks: vec![],
            continues: vec![],
            gen_subj: mark_t.map(|m| (subj_t.to_string(), m.to_string())),
            is_switch: false,
        });
        self.compile_seq(b, body)?;
        let ctx = self.loops.pop().unwrap();

        // `continue` lands here → advance to the next yield.
        let cont_target = b.current_pos();
        self.emit_get_var(b, subj_t);
        b.emit(Op::CallBuiltin(ops::GEN_NEXT, 1), line);
        b.emit(Op::Pop, 0);
        b.emit(Op::Jump(top), 0);

        let end = b.current_pos();
        b.patch_jump(exit, end);
        for j in ctx.breaks {
            b.patch_jump(j, end);
        }
        for j in ctx.continues {
            b.patch_jump(j, cont_target);
        }
        // Leaving the loop (exhausted, or by `break`) frees the iterator. A
        // generator nothing else holds is destroyed there, which runs the
        // `finally` blocks around the `yield` it is suspended at.
        if let Some(m) = mark_t {
            self.emit_gen_release(b, subj_t, m)?;
        }
        Ok(())
    }

    /// Assign a `foreach` destructuring pattern from the temporary the element
    /// was bound to. Emitted at the head of each iteration by both the array and
    /// the generator loop, so `foreach (gen() as [$x, $y])` destructures too.
    ///
    /// The assignment expression leaves its right-hand value on the stack (that
    /// is what makes `$r = [$a, $b] = $src` work), so it is popped here.
    /// `ref_path` is where a `&` target in the pattern must alias — the element
    /// of the SUBJECT being iterated, not the temp the loop bound the row into.
    /// The temp holds a copy for a by-value `foreach`, so a reference to it
    /// would be written and then discarded at the next iteration; PHP's
    /// `foreach ($a as [&$x, $y])` writes through to `$a`. `None` where there is
    /// no such element to point at, as in a generator loop.
    fn emit_foreach_destructure(
        &mut self,
        b: &mut ChunkBuilder,
        pattern: Option<&Expr>,
        val_var: &str,
        ref_path: Option<&Expr>,
    ) -> Result<(), String> {
        let Some(p) = pattern else {
            return Ok(());
        };
        if let Expr::Array(elems, syntax) = p {
            self.check_list_pattern(elems, *syntax)?;
            self.compile_list_targets(b, elems, val_var, ref_path)?;
            return Ok(());
        }
        self.compile_assign(b, p, None, &Expr::Var(val_var.to_string()))?;
        b.emit(Op::Pop, 0);
        Ok(())
    }

    /// The checks the reference makes while COMPILING a class body, member by
    /// member in source order, each a compile-time `Fatal error` (the program
    /// prints nothing): a member declared twice, a method whose body contradicts
    /// its modifiers, a modifier its class kind forbids, and a parameter named
    /// twice. For one method they run in that order — modifiers, then the body,
    /// then the name, then the parameters — which is the order the reference
    /// reports them in when a method breaks several.
    ///
    /// Members are visited in LINE order; two members on one line are visited
    /// constants first, then properties, then methods.
    fn check_class_members(&self, decl: &ClassDecl) -> Result<(), String> {
        enum Member<'a> {
            Const(&'a str, u32),
            Prop(&'a PropDecl),
            Method(&'a Method),
        }
        let class = host::display_class(&decl.name);
        let mut members: Vec<(u32, Member)> = Vec::new();
        for ((name, _), line) in decl.consts.iter().zip(&decl.const_lines) {
            members.push((*line, Member::Const(name, *line)));
        }
        for case in &decl.cases {
            members.push((case.line, Member::Const(&case.name, case.line)));
        }
        for p in &decl.props {
            members.push((p.line, Member::Prop(p)));
        }
        for m in &decl.methods {
            members.push((m.line, Member::Method(m)));
        }
        members.sort_by_key(|(line, _)| *line);

        let mut consts: FxHashSet<&str> = FxHashSet::default();
        let mut props: FxHashSet<&str> = FxHashSet::default();
        let mut methods: FxHashSet<String> = FxHashSet::default();
        for (_, member) in &members {
            match member {
                Member::Const(name, line) => {
                    if !consts.insert(name) {
                        return Err(self.compile_fatal(
                            *line,
                            &format!("Cannot redefine class constant {class}::{name}"),
                        ));
                    }
                }
                Member::Prop(p) => {
                    if !props.insert(&p.name) {
                        return Err(self.compile_fatal(
                            p.line,
                            &format!("Cannot redeclare {class}::${}", p.name),
                        ));
                    }
                    if p.readonly && p.ty.is_none() {
                        let msg = format!("Readonly property {class}::${} must have type", p.name);
                        return Err(self.compile_fatal(p.line, &msg));
                    }
                    if p.readonly && p.is_static {
                        let msg =
                            format!("Static property {class}::${} cannot be readonly", p.name);
                        return Err(self.compile_fatal(p.line, &msg));
                    }
                }
                Member::Method(m) => {
                    let fatal = |msg: String| Err(self.compile_fatal(m.line, &msg));
                    let name = &m.name;
                    if decl.is_interface && m.visibility != Visibility::Public {
                        return fatal(format!(
                            "Access type for interface method {class}::{name}() must be public"
                        ));
                    }
                    if decl.is_enum && m.is_abstract {
                        return fatal(format!(
                            "Enum method {class}::{name}() must not be abstract"
                        ));
                    }
                    if m.is_abstract && !decl.is_interface && !decl.is_abstract && !decl.is_trait {
                        return fatal(format!(
                            "Class {class} declares abstract method {name}() and must therefore be declared abstract"
                        ));
                    }
                    if m.is_abstract
                        && !decl.is_interface
                        && !decl.is_trait
                        && m.visibility == Visibility::Private
                    {
                        return fatal(format!(
                            "Abstract function {class}::{name}() cannot be declared private"
                        ));
                    }
                    if decl.is_interface && m.has_body {
                        return fatal(format!(
                            "Interface function {class}::{name}() cannot contain body"
                        ));
                    }
                    if !decl.is_interface && m.is_abstract && m.has_body {
                        return fatal(format!(
                            "Abstract function {class}::{name}() cannot contain body"
                        ));
                    }
                    if !m.is_abstract && !m.has_body {
                        return fatal(format!(
                            "Non-abstract method {class}::{name}() must contain body"
                        ));
                    }
                    if !methods.insert(name.to_ascii_lowercase()) {
                        return fatal(format!("Cannot redeclare {class}::{name}()"));
                    }
                    self.check_params(&m.params)?;
                    if name.eq_ignore_ascii_case("__construct") {
                        for p in m.params.iter().filter(|p| p.promoted) {
                            if !props.insert(&p.name) {
                                return Err(self.compile_fatal(
                                    p.line,
                                    &format!("Cannot redeclare {class}::${}", p.name),
                                ));
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// The `final` rules the class just compiled from `decl` must keep against
    /// what it inherits — see [`host::final_violation`]. An EARLY-bound class
    /// links before the file runs, so a broken rule is a compile-time fatal;
    /// any other links where its declaration runs, so the check is an op there
    /// (`ops::FINAL_CHECK`), made against the class table as it is by then.
    fn check_finals(
        &mut self,
        b: &mut ChunkBuilder,
        decl: &ClassDecl,
        early: bool,
        line: u32,
    ) -> Result<(), String> {
        if self.prelude || (decl.parent.is_none() && decl.implements.is_empty()) {
            return Ok(());
        }
        if early {
            let def = &self
                .classes
                .last()
                .expect("the class was just registered")
                .1;
            // Outside `with_host`: the first call compiles the prelude.
            let prelude = crate::prelude_classes();
            let local = |n: &str| {
                let lname = n.to_ascii_lowercase();
                self.find_class(n)
                    .or_else(|| prelude.iter().find(|(k, _)| *k == lname).map(|(_, d)| d))
            };
            let broken = host::with_host(|h| h.final_check_with(def, |n| local(n)));
            return match broken {
                Some((msg, at)) => Err(self.compile_fatal(at.unwrap_or(line), &msg)),
                None => Ok(()),
            };
        }
        let idx = b.add_constant(Value::str(decl.name.to_ascii_lowercase()));
        b.emit(Op::LoadConst(idx), line);
        b.emit(Op::CallBuiltin(ops::FINAL_CHECK, 1), line);
        b.emit(Op::Pop, line);
        Ok(())
    }

    /// A parameter list that names one parameter twice is the reference's
    /// compile-time `Redefinition of parameter $x`, reported at the second.
    /// The deprecations `zend_compile_params` raises over parameter defaults,
    /// one parameter at a time in declared order, and the index of the last
    /// required parameter — every default ahead of it is discarded, so those
    /// parameters become required (`exactly N expected`, `Argument #1 ($a) not
    /// passed`).
    ///
    /// A typed parameter defaulting to a literal `null` whose type does not
    /// admit null is the old spelling of `?T`: it raises `Implicitly marking
    /// parameter $a as nullable is deprecated` wherever it stands, and is
    /// exempt from `Optional parameter $a declared before required parameter
    /// $b is implicitly treated as a required parameter`, which every other
    /// default ahead of the last required parameter raises.
    fn check_param_defaults(&self, params: &[Param], owner: &str) -> usize {
        let last_required = params
            .iter()
            .rposition(|p| p.default.is_none() && !p.variadic);
        if self.prelude {
            return last_required.unwrap_or(0);
        }
        for (i, p) in params.iter().enumerate() {
            if p.default.is_none() {
                continue;
            }
            let msg = if implicitly_nullable(p) {
                format!(
                    "{owner}: Implicitly marking parameter ${} as nullable is deprecated, the \
                     explicit nullable type must be used instead",
                    p.name
                )
            } else if let Some(r) = last_required.filter(|&r| i < r) {
                format!(
                    "{owner}: Optional parameter ${} declared before required parameter ${} is \
                     implicitly treated as a required parameter",
                    p.name, params[r].name
                )
            } else {
                continue;
            };
            crate::lexer::push_diag("Deprecated", crate::errlevel::E_DEPRECATED, p.line, msg);
        }
        last_required.unwrap_or(0)
    }

    /// `zend_emit_return_type_check`'s compile errors for a `return` in a body
    /// declared `void` (one that carries a value) or `never` (any). A body in a
    /// class — including a closure written in a method — is a "method".
    fn return_refusal(&self, e: Option<&Expr>) -> Option<String> {
        let kind = if self.current_class.is_some() {
            "method"
        } else {
            "function"
        };
        match (self.ret_rule?, e) {
            ("void", Some(Expr::Null)) => Some(format!(
                "A void {kind} must not return a value (did you mean \"return;\" instead of \
                 \"return null;\"?)"
            )),
            ("void", Some(_)) => Some(format!("A void {kind} must not return a value")),
            ("never", _) => Some(format!("A never-returning {kind} must not return")),
            _ => None,
        }
    }

    /// What `zend_compile_closure_binding` and `zend_compile_closure_uses` refuse
    /// in a `use (...)` clause: `$this` and a superglobal, a name listed twice,
    /// and a name that is also a parameter.
    fn check_closure_uses(
        &self,
        params: &[Param],
        uses: &[Capture],
        line: u32,
    ) -> Result<(), String> {
        let mut seen: FxHashSet<&str> = FxHashSet::default();
        for u in uses {
            let msg = if u.name == "this" {
                "Cannot use $this as lexical variable".to_string()
            } else if is_auto_global(&u.name) {
                "Cannot use auto-global as lexical variable".to_string()
            } else if !seen.insert(&u.name) {
                format!("Cannot use variable ${} twice", u.name)
            } else {
                continue;
            };
            return Err(self.compile_fatal(line, &msg));
        }
        if let Some(u) = uses
            .iter()
            .find(|u| params.iter().any(|p| p.name == u.name))
        {
            let msg = format!(
                "Cannot use lexical variable ${} as a parameter name",
                u.name
            );
            return Err(self.compile_fatal(line, &msg));
        }
        Ok(())
    }

    /// What the reference refuses in an `enum` declaration, in the order it
    /// finds it: the backing type (at the declaration), then the members as
    /// they are compiled — a case whose value disagrees with the enum being
    /// backed or not, a property — and last, once the body is done, a magic
    /// method an enum may not have.
    fn check_enum(&self, decl: &ClassDecl) -> Result<(), String> {
        if !decl.is_enum {
            return Ok(());
        }
        let (name, line) = (&decl.name, self.cur_line);
        if let Some(t) = &decl.enum_backing {
            if !t.eq_ignore_ascii_case("int") && !t.eq_ignore_ascii_case("string") {
                let msg = format!("Enum backing type must be int or string, {t} given");
                return Err(self.compile_fatal(line, &msg));
            }
        }
        let backed = decl.enum_backing.is_some();
        let cases = decl.cases.iter().filter_map(|c| match (backed, &c.value) {
            (true, None) => Some((
                c.line,
                format!("Case {} of backed enum {name} must have a value", c.name),
            )),
            (false, Some(_)) => Some((
                c.line,
                format!(
                    "Case {} of non-backed enum {name} must not have a value",
                    c.name
                ),
            )),
            _ => None,
        });
        let props = decl
            .props
            .iter()
            .map(|p| (p.line, format!("Enum {name} cannot include properties")));
        if let Some((at, msg)) = cases.chain(props).min_by_key(|(at, _)| *at) {
            return Err(self.compile_fatal(at, &msg));
        }
        // `zend_verify_enum_magic_methods` checks in this fixed order, and names
        // the method in this spelling whatever the declaration wrote.
        const BANNED: [&str; 14] = [
            "__construct",
            "__destruct",
            "__clone",
            "__get",
            "__set",
            "__unset",
            "__isset",
            "__toString",
            "__debugInfo",
            "__serialize",
            "__unserialize",
            "__sleep",
            "__wakeup",
            "__set_state",
        ];
        let has = |n: &str| decl.methods.iter().any(|m| m.name.eq_ignore_ascii_case(n));
        if let Some(m) = BANNED.iter().find(|n| has(n)) {
            let msg = format!("Enum {name} cannot include magic method {m}");
            return Err(self.compile_fatal(line, &msg));
        }
        Ok(())
    }

    /// The per-parameter compile errors of `zend_compile_params`, in its order.
    fn check_params(&self, params: &[Param]) -> Result<(), String> {
        let mut seen: FxHashSet<&str> = FxHashSet::default();
        for (i, p) in params.iter().enumerate() {
            let fatal = |msg: &str| Err(self.compile_fatal(p.line, msg));
            if is_auto_global(&p.name) {
                return fatal(&format!("Cannot re-assign auto-global variable {}", p.name));
            }
            if !seen.insert(&p.name) {
                return fatal(&format!("Redefinition of parameter ${}", p.name));
            }
            if p.name == "this" {
                return fatal("Cannot use $this as parameter");
            }
            if i > 0 && params[i - 1].variadic {
                return fatal("Only the last parameter can be variadic");
            }
            if p.variadic && p.default.is_some() {
                return fatal("Variadic parameter cannot have a default value");
            }
            let parts = p.ty.as_ref().map_or(&[][..], |t| t.parts.as_slice());
            // The two spellings are the reference's own: `Void` is capitalized.
            for (bottom, in_union) in [("void", "Void"), ("never", "never")] {
                if parts.iter().any(|t| t.eq_ignore_ascii_case(bottom)) {
                    return fatal(&if parts.len() > 1 {
                        format!("{in_union} can only be used as a standalone type")
                    } else {
                        format!("{bottom} cannot be used as a parameter type")
                    });
                }
            }
        }
        Ok(())
    }

    /// Lower a class declaration to a `ClassDef`: constant and property-default
    /// initializers become standalone expression chunks (each leaving its value
    /// on the stack), and each method body compiles like a free function. A
    /// constructor with promoted parameters (`public int $x`) gets a synthetic
    /// `$this->x = $x;` prepended for each promoted parameter.
    fn compile_class(&mut self, b: &mut ChunkBuilder, decl: &ClassDecl) -> Result<(), String> {
        if !self.prelude {
            self.check_class_members(decl)?;
            self.check_enum(decl)?;
        }
        let prev_class = self.current_class.take();
        let prev_parent = self.current_parent.take();
        let prev_in_trait = std::mem::replace(&mut self.in_trait, decl.is_trait);
        self.current_class = Some(decl.name.clone());
        self.current_parent = decl.parent.clone();

        // Seed members from any used traits (declared earlier); the class's own
        // members below override them, matching PHP trait precedence.
        let mut consts: Vec<(String, Chunk)> = Vec::new();
        let mut const_vis: FxHashMap<String, Visibility> = FxHashMap::default();
        let mut prop_defaults: Vec<(String, Chunk)> = Vec::new();
        let mut static_prop_defaults: Vec<(String, Chunk)> = Vec::new();
        let mut methods: FxHashMap<String, FuncDef> = FxHashMap::default();
        let mut prop_vis: FxHashMap<String, Visibility> = FxHashMap::default();
        let mut readonly_props: FxHashSet<String> = FxHashSet::default();
        let mut uninit_props: FxHashMap<String, String> = FxHashMap::default();
        let mut prop_types: FxHashMap<String, TypeHint> = FxHashMap::default();
        let mut method_vis: FxHashMap<String, Visibility> = FxHashMap::default();
        let mut static_methods: FxHashSet<String> = FxHashSet::default();
        let mut finals = Finals::default();
        let mut order: Vec<String> = Vec::new();
        match self.seed_from_traits(
            decl,
            &mut consts,
            &mut const_vis,
            &mut prop_defaults,
            &mut static_prop_defaults,
            &mut methods,
            &mut prop_vis,
            &mut readonly_props,
            &mut method_vis,
            &mut static_methods,
            &mut order,
            &mut finals,
        ) {
            Ok(()) => {}
            // A bad `use` is a *link*-time failure in PHP, not a compile-time
            // one: everything the script printed before the declaration is
            // printed first. So it is emitted at the declaration's own place in
            // the instruction stream rather than failing the compile, and the
            // class is left unregistered — the program never gets past it.
            Err(err) => {
                match err {
                    LinkError::Fatal(msg) => {
                        let idx = b.add_constant(Value::str(msg));
                        b.emit(Op::LoadConst(idx), self.cur_line);
                        b.emit(Op::CallBuiltin(ops::DECL_FATAL, 1), self.cur_line);
                    }
                    // A missing trait, unlike a conflict, is an ordinary
                    // throwable `Error` — an `eval`'d declaration can be caught.
                    LinkError::Throw(msg) => {
                        let e = Expr::Throw(Box::new(Expr::New(
                            "Error".to_string(),
                            vec![Expr::Str(msg)],
                        )));
                        self.compile_expr(b, &e)?;
                    }
                }
                b.emit(Op::Pop, self.cur_line);
                self.current_class = prev_class;
                self.in_trait = prev_in_trait;
                self.current_parent = prev_parent;
                return Ok(());
            }
        }

        for (name, expr) in &decl.consts {
            let mut cb = ChunkBuilder::new();
            self.in_other_frame(|c| c.compile_expr(&mut cb, expr))?;
            consts.retain(|(n, _)| n != name);
            consts.push((name.clone(), cb.build()));
            const_vis.remove(name);
            finals.consts.remove(name);
        }
        finals.consts.extend(decl.final_consts.iter().cloned());
        for (name, vis) in &decl.const_vis {
            const_vis.insert(name.clone(), *vis);
        }

        // A trait's default-less typed properties arrive with it.
        for tname in &decl.uses {
            if let Some(t) = self.find_class(tname) {
                uninit_props.extend(t.uninit_props.iter().map(|(n, ty)| (n.clone(), ty.clone())));
                prop_types.extend(t.prop_types.iter().map(|(n, ty)| (n.clone(), ty.clone())));
            }
        }
        for prop in &decl.props {
            let name = &prop.name;
            prop_vis.insert(name.clone(), prop.visibility);
            if prop.is_final {
                finals.props.insert(name.clone());
            } else {
                finals.props.remove(name);
            }
            match &prop.ty {
                Some(ty) => prop_types.insert(name.clone(), ty.clone()),
                None => prop_types.remove(name),
            };
            match (&prop.ty, &prop.default, prop.is_static) {
                (Some(ty), None, false) => {
                    let display = ty.declared(&decl.name, decl.parent.as_deref());
                    uninit_props.insert(name.clone(), display);
                }
                _ => {
                    uninit_props.remove(name);
                }
            }
            // A static property cannot be readonly (PHP rejects the pair at
            // compile time), so only instance declarations register one.
            if prop.readonly && !prop.is_static {
                readonly_props.insert(name.clone());
            }
            let mut pb = ChunkBuilder::new();
            match &prop.default {
                Some(e) => self.in_other_frame(|c| c.compile_expr(&mut pb, e))?,
                None => {
                    pb.emit(Op::LoadUndef, 0);
                }
            }
            // Static properties are class-level (never copied into an instance);
            // instance properties become per-object defaults.
            if prop.is_static {
                static_prop_defaults.retain(|(n, _)| n != name);
                static_prop_defaults.push((name.clone(), pb.build()));
            } else {
                prop_defaults.retain(|(n, _)| n != name);
                prop_defaults.push((name.clone(), pb.build()));
            }
        }
        // A class's own properties take their slots before the ones its traits
        // bring in: `zend_do_bind_traits` adds trait properties after the class
        // body is declared. The sort is stable, so each group keeps its order.
        prop_defaults
            .sort_by_key(|(n, _)| !decl.props.iter().any(|p| !p.is_static && &p.name == n));

        for m in &decl.methods {
            method_vis.insert(m.name.to_ascii_lowercase(), m.visibility);
            if m.is_static {
                static_methods.insert(m.name.to_ascii_lowercase());
            }
            order.retain(|n| !n.eq_ignore_ascii_case(&m.name));
            order.push(m.name.clone());
            finals
                .method_sites
                .retain(|(n, _)| !n.eq_ignore_ascii_case(&m.name));
            finals.method_sites.push((m.name.clone(), m.line));
            if m.is_final {
                finals.methods.insert(m.name.to_ascii_lowercase());
            } else {
                finals.methods.remove(&m.name.to_ascii_lowercase());
            }
            let owner = if decl.namespace.is_empty() {
                format!("{}::{}()", decl.name, m.name)
            } else {
                format!("{}\\{}::{}()", decl.namespace, decl.name, m.name)
            };
            let cparams = self.compile_params(&m.params, &owner)?;
            let mut mb = ChunkBuilder::new();
            // A method body has its own loop scope (as free functions do).
            let saved = std::mem::take(&mut self.loops);
            let saved_try = self.enter_own_loop_scope();
            // Constructor property promotion: `public int $x` also assigns
            // `$this->x = $x` before the body runs.
            let mut promotions: Vec<Expr> = Vec::new();
            if m.name.eq_ignore_ascii_case("__construct") {
                for p in m.params.iter().filter(|p| p.promoted) {
                    // A promoted parameter DECLARES the property, so the synthetic
                    // assignment below must not read as creating a dynamic one.
                    prop_vis.insert(p.name.clone(), p.promoted_vis);
                    if let Some(ty) = &p.ty {
                        prop_types.insert(p.name.clone(), ty.clone());
                    }
                    if p.readonly || decl.is_readonly {
                        readonly_props.insert(p.name.clone());
                    }
                    promotions.push(Expr::Assign(
                        Box::new(Expr::PropGet(
                            Box::new(Expr::Var("this".to_string())),
                            Member::Name(p.name.clone()),
                        )),
                        None,
                        Box::new(Expr::Var(p.name.clone())),
                    ));
                }
            }
            let saved_ref = std::mem::replace(&mut self.ret_by_ref, m.by_ref_return);
            let saved_rule =
                std::mem::replace(&mut self.ret_rule, ret_rule(m.ret.as_ref(), &m.body));
            // A closure written in a method body is `{closure:Class::method():LINE}`
            // whether the method is static or not — PHP always spells the
            // enclosing method with `::` here, even though the FRAME above it
            // uses `->` for an instance call.
            let saved_site = std::mem::replace(
                &mut self.decl_site,
                host::DeclSite::Named(format!("{}::{}()", decl.name, m.name)),
            );
            // The promotions run in the CONSTRUCTOR's frame, so they are lowered
            // inside it. Compiled outside, `$x` on the right-hand side was
            // numbered against the ENCLOSING scope's slots — so a top-level
            // variable of the same name as a promoted parameter made
            // `$this->x = $x` read whatever the constructor frame happened to
            // hold at that index instead of the parameter.
            self.in_other_frame(|c| {
                for assign in &promotions {
                    c.compile_expr(&mut mb, assign)?;
                    mb.emit(Op::Pop, 0);
                }
                c.compile_seq(&mut mb, &m.body)
            })?;
            self.resolve_gotos(&mut mb, self.cur_chunk, true)?;
            self.decl_site = saved_site;
            self.ret_by_ref = saved_ref;
            self.loops = saved;
            self.leave_own_loop_scope(saved_try);
            self.ret_rule = saved_rule;
            methods.insert(
                m.name.to_ascii_lowercase(),
                FuncDef {
                    params: cparams,
                    // A method's frame is named by the method.
                    closure_site: None,
                    declared: None,
                    chunk: mb.build(),
                    deprecated: m.deprecated.clone(),
                    is_generator: body_has_yield(&m.body),
                    ret: m.ret.clone(),
                    // Methods keep the by-name path for now; `$this` and the
                    // property desugaring bind through the host either way.
                    locals: Vec::new(),
                },
            );
        }

        // An `enum`'s cases: each case's optional backing-value expression is
        // lowered to its own chunk (run once when the singleton is built).
        let mut enum_cases: Vec<(String, Option<Chunk>)> = Vec::new();
        for case in &decl.cases {
            let chunk = match &case.value {
                Some(e) => {
                    let mut cb = ChunkBuilder::new();
                    self.in_other_frame(|c| c.compile_expr(&mut cb, e))?;
                    Some(cb.build())
                }
                None => None,
            };
            enum_cases.push((case.name.clone(), chunk));
        }

        // A private method is never inherited, so its `final` binds nothing —
        // except on the constructor, which the reference checks regardless.
        finals
            .methods
            .retain(|m| m == "__construct" || method_vis.get(m) != Some(&Visibility::Private));
        self.method_order
            .insert(decl.name.to_ascii_lowercase(), order);
        // The reference's function table holds the class's own methods in
        // declaration order, then the methods its traits add: the traits were
        // seeded first here, so the class's own move ahead of them.
        let (mut own, traits): (Vec<_>, Vec<_>) = std::mem::take(&mut finals.method_sites)
            .into_iter()
            .partition(|(n, _)| decl.methods.iter().any(|m| m.name.eq_ignore_ascii_case(n)));
        own.extend(traits);
        finals.method_sites = own;
        self.classes.push((
            decl.name.to_ascii_lowercase(),
            ClassDef {
                name: decl.name.clone(),
                parent: decl.parent.clone(),
                interfaces: decl.implements.clone(),
                consts,
                const_vis,
                prop_defaults,
                static_prop_defaults,
                methods,
                prop_vis,
                readonly_props,
                uninit_props,
                prop_types,
                method_vis,
                static_methods,
                is_enum: decl.is_enum,
                is_abstract: decl.is_abstract,
                is_interface: decl.is_interface,
                is_trait: decl.is_trait,
                uses: decl.uses.clone(),
                allow_dynamic_props: decl
                    .attributes
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case("AllowDynamicProperties")),
                enum_cases,
                is_final: decl.is_final,
                final_methods: finals.methods,
                final_consts: finals.consts,
                final_props: finals.props,
                method_sites: finals.method_sites,
            },
        ));

        self.current_class = prev_class;
        self.in_trait = prev_in_trait;
        self.current_parent = prev_parent;
        Ok(())
    }

    /// Merge the members of every trait named by `use` into the tables the class
    /// is being built from, applying the `insteadof`/`as` adaptations.
    ///
    /// `Err` is the *body* of a PHP link-time fatal error (no severity, no
    /// location — the caller's op adds those). Every one of them is a case PHP
    /// refuses to link, so returning early with the class unregistered is right:
    /// the program stops at the declaration.
    ///
    /// Two traits declaring the same method is such a case unless an `insteadof`
    /// picks a winner — silently taking the last one, which is what a flat merge
    /// does, hides a real conflict.
    #[allow(clippy::too_many_arguments)]
    fn seed_from_traits(
        &self,
        decl: &ClassDecl,
        consts: &mut Vec<(String, Chunk)>,
        const_vis: &mut FxHashMap<String, Visibility>,
        prop_defaults: &mut Vec<(String, Chunk)>,
        static_prop_defaults: &mut Vec<(String, Chunk)>,
        methods: &mut FxHashMap<String, FuncDef>,
        prop_vis: &mut FxHashMap<String, Visibility>,
        readonly_props: &mut FxHashSet<String>,
        method_vis: &mut FxHashMap<String, Visibility>,
        static_methods: &mut FxHashSet<String>,
        order: &mut Vec<String>,
        finals: &mut Finals,
    ) -> Result<(), LinkError> {
        if decl.uses.is_empty() {
            return Ok(());
        }
        // The used traits, in `use` order. A name that never got declared cannot
        // be linked against at all.
        let mut used: Vec<&ClassDef> = Vec::with_capacity(decl.uses.len());
        for tname in &decl.uses {
            match self.find_class(tname) {
                Some(d) => used.push(d),
                // A `use` naming something that was never declared is the one
                // link failure the reference reports as a throwable `Error`.
                None => return Err(LinkError::Throw(format!("Trait \"{tname}\" not found"))),
            }
        }
        // Every trait named on either side of an adaptation must be one of them.
        let mut named = Vec::new();
        for ins in &decl.trait_insteadof {
            named.push(&ins.winner);
            named.extend(ins.losers.iter());
        }
        named.extend(decl.trait_aliases.iter().filter_map(|a| a.from.as_ref()));
        for tname in named {
            if !used.iter().any(|d| d.name.eq_ignore_ascii_case(tname)) {
                return Err(LinkError::Fatal(match self.find_class(tname) {
                    Some(_) => format!("Required Trait {tname} wasn't added to {}", decl.name),
                    None => format!("Could not find trait {tname}"),
                }));
            }
        }

        // Non-method members merge flat: `insteadof`/`as` only ever speak about
        // methods, and a class's own declaration overrides whatever a trait
        // brought in (that override happens in the caller, after this).
        for tdef in &used {
            consts.extend(tdef.consts.iter().cloned());
            for (n, v) in &tdef.const_vis {
                const_vis.insert(n.clone(), *v);
            }
            prop_defaults.extend(tdef.prop_defaults.iter().cloned());
            static_prop_defaults.extend(tdef.static_prop_defaults.iter().cloned());
            for (n, v) in &tdef.prop_vis {
                prop_vis.insert(n.clone(), *v);
            }
            // A property a trait declares readonly stays readonly in the class
            // that uses it — the trait is where it was declared.
            readonly_props.extend(tdef.readonly_props.iter().cloned());
            finals.consts.extend(tdef.final_consts.iter().cloned());
            finals.props.extend(tdef.final_props.iter().cloned());
        }

        // `A::m insteadof B` drops B's `m` from consideration; A's is not
        // "chosen" so much as B's is excluded, which is why three traits still
        // collide when only one of them is excluded.
        let excluded: Vec<(String, String)> = decl
            .trait_insteadof
            .iter()
            .flat_map(|ins| {
                ins.losers
                    .iter()
                    .map(|l| (l.to_ascii_lowercase(), ins.method.to_ascii_lowercase()))
            })
            .collect();
        let is_excluded = |tdef: &ClassDef, m: &str| {
            excluded
                .iter()
                .any(|(t, em)| tdef.name.eq_ignore_ascii_case(t) && em == m)
        };

        // Every method name any used trait declares, in trait order then
        // declaration order, so the collision reported is the first one PHP
        // would reach.
        let mut seen: Vec<String> = Vec::new();
        for tdef in &used {
            // A trait loaded by an earlier compilation has no recorded
            // declaration order here; its method table, sorted, stands in.
            let mut keys: Vec<String> = self
                .declared_methods(&tdef.name)
                .iter()
                .map(|s| s.to_ascii_lowercase())
                .collect();
            if keys.is_empty() {
                keys = tdef.methods.keys().cloned().collect();
                keys.sort();
            }
            for key in keys {
                if !seen.contains(&key) {
                    seen.push(key);
                }
            }
        }
        for m in &seen {
            let candidates: Vec<&&ClassDef> = used
                .iter()
                .filter(|t| t.methods.contains_key(m) && !is_excluded(t, m))
                .collect();
            let Some(winner) = candidates.first() else {
                continue;
            };
            // PHP names the SECOND candidate as the one not applied and the
            // first as the one it collided with, whatever the pair's position.
            if let Some(loser) = candidates.get(1) {
                let kept = self.method_spelling(&winner.name, m);
                let dropped = self.method_spelling(&loser.name, m);
                return Err(LinkError::Fatal(format!(
                    "Trait method {}::{dropped} has not been applied as {}::{dropped}, \
                     because of collision with {}::{kept}",
                    loser.name, decl.name, winner.name
                )));
            }
            methods.insert(m.clone(), winner.methods[m].clone());
            if let Some(v) = winner.method_vis.get(m) {
                method_vis.insert(m.clone(), *v);
            }
            if winner.static_methods.contains(m) {
                static_methods.insert(m.clone());
            }
            order.push(self.method_spelling(&winner.name, m));
            if winner.final_methods.contains(m) {
                finals.methods.insert(m.clone());
            }
            if let Some(site) = winner
                .method_sites
                .iter()
                .find(|(s, _)| s.eq_ignore_ascii_case(m))
            {
                finals.method_sites.push(site.clone());
            }
        }

        // Aliases resolve against each trait's OWN method table, not the merged
        // one: `A::hi insteadof B; B::hi as bHi;` is the whole point of the
        // construct, and B's `hi` is excluded from the merge.
        for al in &decl.trait_aliases {
            let key = al.method.to_ascii_lowercase();
            let source = match &al.from {
                Some(tname) => {
                    let tdef = used
                        .iter()
                        .find(|d| d.name.eq_ignore_ascii_case(tname))
                        .expect("adaptation trait names were checked above");
                    if !tdef.methods.contains_key(&key) {
                        return Err(LinkError::Fatal(format!(
                            "An alias was defined for {tname}::{} but this method does not exist",
                            al.method
                        )));
                    }
                    *tdef
                }
                None => {
                    let found: Vec<&&ClassDef> = used
                        .iter()
                        .filter(|d| d.methods.contains_key(&key))
                        .collect();
                    match found.as_slice() {
                        [one] => **one,
                        [] => {
                            let alias = al.alias.clone().unwrap_or_else(|| al.method.clone());
                            return Err(LinkError::Fatal(format!(
                                "An alias ({alias}) was defined for method {}(), but this \
                                 method does not exist",
                                al.method
                            )));
                        }
                        [a, b, ..] => {
                            let m = self.method_spelling(&a.name, &key);
                            return Err(LinkError::Fatal(format!(
                                "An alias was defined for method {m}(), which exists in both \
                                 {} and {}. Use {}::{m} or {}::{m} to resolve the ambiguity",
                                a.name, b.name, a.name, b.name
                            )));
                        }
                    }
                }
            };
            match &al.alias {
                // With a new name the method gains a SECOND binding; the
                // original one stays exactly as it was.
                Some(alias) => {
                    let ak = alias.to_ascii_lowercase();
                    methods.insert(ak.clone(), source.methods[&key].clone());
                    let vis = al
                        .visibility
                        .or_else(|| source.method_vis.get(&key).copied())
                        .unwrap_or(Visibility::Public);
                    method_vis.insert(ak.clone(), vis);
                    if source.final_methods.contains(&key) {
                        finals.methods.insert(ak.clone());
                    }
                    if let Some((_, line)) = source
                        .method_sites
                        .iter()
                        .find(|(s, _)| s.eq_ignore_ascii_case(&key))
                    {
                        finals.method_sites.push((alias.clone(), *line));
                    }
                    if source.static_methods.contains(&key) {
                        static_methods.insert(ak);
                    }
                    order.push(alias.clone());
                }
                // Without one, only the visibility of the existing binding moves.
                None => {
                    if let Some(v) = al.visibility {
                        method_vis.insert(key, v);
                    }
                }
            }
        }
        Ok(())
    }

    /// The name PHP gives an anonymous class:
    /// `Base@anonymous\0<script>:<line>$<n>`, where `Base` is the parent class,
    /// else the first implemented interface, else the literal `class`, and `n`
    /// is a hexadecimal per-compilation counter.
    ///
    /// The NUL is not a quirk of this port — the reference builds the name that
    /// way so that everything printing it as a C string (`var_dump`, `print_r`)
    /// shows only the readable head, while `get_class` returns the whole,
    /// guaranteed-unique string.
    fn anon_class_name(&mut self, decl: &ClassDecl, line: u32) -> String {
        let base = decl
            .parent
            .as_deref()
            .or(decl.implements.first().map(String::as_str))
            .unwrap_or("class");
        let n = self.anon_classes;
        self.anon_classes += 1;
        let script = host::with_host(|h| h.script_name().to_string());
        format!("{base}@anonymous\0{script}:{line}${n:x}")
    }

    /// A declared class/interface/trait by name, case-insensitively.
    fn find_class(&self, name: &str) -> Option<&ClassDef> {
        let key = name.to_ascii_lowercase();
        self.classes
            .iter()
            .chain(self.known_traits.iter())
            // A type declared inside a block waits under a key of its own
            // until it runs, so it is found by its declared name.
            .find(|(n, d)| *n == key || d.name.eq_ignore_ascii_case(name))
            .map(|(_, d)| d)
    }

    /// The method names a class declared, in their source spelling and source
    /// order — see [`Compiler::method_order`].
    fn declared_methods(&self, class: &str) -> &[String] {
        self.method_order
            .get(&class.to_ascii_lowercase())
            .map_or(&[], Vec::as_slice)
    }

    /// How `class` spelled the method whose lowercased name is `key`. Falls back
    /// to the lowercased form, which is only reachable for a method no
    /// declaration recorded.
    fn method_spelling(&self, class: &str, key: &str) -> String {
        self.declared_methods(class)
            .iter()
            .find(|n| n.eq_ignore_ascii_case(key))
            .cloned()
            .unwrap_or_else(|| key.to_string())
    }

    /// Resolve a class reference to a concrete name, expanding the `self`,
    /// `parent`, and `static` keywords against the class being compiled.
    /// Push the class a `Class::…` / `new Class` names.
    ///
    /// `self` and `parent` are fixed at compile time, but `static` is *late* —
    /// it names the class the running call was made on, which only the frame
    /// knows — so it pushes a runtime lookup instead of a constant. The enclosing
    /// class travels along as the fallback for a `static::` reached outside any
    /// method call.
    fn emit_class_name(&mut self, b: &mut ChunkBuilder, class: &str) -> Result<(), String> {
        self.emit_class_name_as(b, class, false)
    }

    /// [`Compiler::emit_class_name`], told whether the reference is `X::class`.
    ///
    /// That form is its own opcode in the reference (`ZEND_FETCH_CLASS_NAME`)
    /// and words its refusals differently: `Cannot use "self" in the global
    /// scope` where every other fetch says `Cannot access "self" when no class
    /// scope is active`. The two differ only where the scope is decided at run
    /// time, so only the unbound-closure path below carries the flag.
    fn emit_class_name_as(
        &mut self,
        b: &mut ChunkBuilder,
        class: &str,
        name_fetch: bool,
    ) -> Result<(), String> {
        // Inside a TRAIT, `self` and `parent` are not knowable here: the body is
        // compiled once and copied into every class that uses it, so the class
        // they name is whichever one ends up running the method. Resolved at run
        // time from the frame, the way `__CLASS__` already is (see the trait arm
        // of `Parser::class_stmt`). `static` needs no special case — it is
        // already a run-time lookup.
        if self.in_trait {
            match class.to_ascii_lowercase().as_str() {
                "self" => {
                    b.emit(Op::CallBuiltin(ops::SELF_CLASS, 0), self.cur_line);
                    return Ok(());
                }
                "parent" => {
                    b.emit(Op::CallBuiltin(ops::PARENT_CLASS, 0), self.cur_line);
                    return Ok(());
                }
                _ => {}
            }
        }
        // A CLOSURE (or the top level) with no enclosing class is the other
        // scope that is not knowable here: `Closure::bind` and `->call()` give
        // one a class later, so `(function () { return self::K; })->call($o)`
        // resolves against the bound scope and only fails — with a catchable
        // `Error` — if it is called without one. Refusing it at compile time
        // rejected a program the reference runs. A NAMED function is the case
        // the reference itself settles at compile time, and keeps the
        // compile-time path below.
        // A closure written INSIDE a class is no different: its scope is the
        // one it was created or bound under, so `Closure::bind($f, null, B::class)`
        // makes its `self` name `B`.
        let unbound_closure = matches!(self.decl_site, host::DeclSite::Closure(..))
            || (self.current_class.is_none()
                && !matches!(self.decl_site, host::DeclSite::Named(_)));
        if unbound_closure {
            let lower = class.to_ascii_lowercase();
            if matches!(lower.as_str(), "self" | "parent" | "static") {
                let kw = b.add_constant(Value::str(lower.clone()));
                b.emit(Op::LoadConst(kw), self.cur_line);
                let op = if lower == "parent" {
                    ops::PARENT_CLASS
                } else {
                    ops::SELF_CLASS
                };
                let argc = if name_fetch {
                    let flag = b.add_constant(Value::Bool(true));
                    b.emit(Op::LoadConst(flag), self.cur_line);
                    2
                } else {
                    1
                };
                b.emit(Op::CallBuiltin(op, argc), self.cur_line);
                // `static` is the late-static-binding class when one is set and
                // the bound scope otherwise, which is what `LSB_CLASS` decides.
                if lower == "static" {
                    b.emit(Op::CallBuiltin(ops::LSB_CLASS, 1), self.cur_line);
                }
                return Ok(());
            }
        }
        let cname = self.resolve_class_name(class)?;
        let idx = b.add_constant(Value::str(cname));
        b.emit(Op::LoadConst(idx), 0);
        if class.eq_ignore_ascii_case("static") {
            b.emit(Op::CallBuiltin(ops::LSB_CLASS, 1), 0);
        }
        Ok(())
    }

    /// Push the class a `::` names. The dynamic form is knowable only at run
    /// time, so the expression is compiled and `DYN_CLASS` turns its value into
    /// a class name at the point of use.
    fn emit_class_ref(&mut self, b: &mut ChunkBuilder, class: &ClassRef) -> Result<(), String> {
        match class {
            ClassRef::Name(n) => self.emit_class_name(b, n),
            ClassRef::Expr(e) => {
                self.compile_expr(b, e)?;
                b.emit(Op::CallBuiltin(ops::DYN_CLASS, 1), self.cur_line);
                Ok(())
            }
        }
    }

    /// Emit the forwarding marker for a `self::` / `parent::` / `static::` call,
    /// which keeps the caller's late-static-binding class rather than replacing
    /// it with the class the call names. Naming a class explicitly does not
    /// forward, so nothing is emitted for it.
    fn emit_lsb_forward(&mut self, b: &mut ChunkBuilder, class: &ClassRef) {
        let lower = class.name().unwrap_or_default().to_ascii_lowercase();
        if matches!(lower.as_str(), "self" | "parent" | "static") {
            b.emit(Op::CallBuiltin(ops::LSB_FORWARD, 0), 0);
            b.emit(Op::Pop, 0);
        }
    }

    fn resolve_class_name(&self, name: &str) -> Result<String, String> {
        match name.to_ascii_lowercase().as_str() {
            "self" | "static" => self
                .current_class
                .clone()
                .ok_or_else(|| format!("'{name}' used outside of a class")),
            "parent" => self
                .current_parent
                .clone()
                .ok_or_else(|| "'parent' used in a class with no parent".to_string()),
            _ => Ok(name.to_string()),
        }
    }

    /// Compile a `body` into its own detached chunk (its own loop scope, and a
    /// `try`-body context so a bare `break`/`continue` becomes a control signal
    /// relayed by the orchestrator to the enclosing loop).
    fn compile_detached(&mut self, body: &[Stmt]) -> Result<Chunk, String> {
        let mut fb = ChunkBuilder::new();
        // A detached body must not see the enclosing loop's break/continue
        // fixups — those live in the parent chunk, unreachable from here.
        let saved = std::mem::take(&mut self.loops);
        // Put back by value, not by undoing the increments: a body that fails
        // to compile returns early from a nested function scope without
        // restoring what that scope zeroed.
        let around = (self.in_try, self.outer_loops.clone());
        self.outer_loops.extend(saved.iter().map(|l| l.is_switch));
        self.chunk_seq += 1;
        let chunk = std::mem::replace(&mut self.cur_chunk, self.chunk_seq);
        self.in_try += 1;
        let r = self.compile_seq(&mut fb, body);
        (self.in_try, self.outer_loops) = around;
        self.loops = saved;
        r?;
        let id = std::mem::replace(&mut self.cur_chunk, chunk);
        self.resolve_gotos(&mut fb, id, false)?;
        Ok(fb.build())
    }

    /// `try { } catch (T|U $e) { } finally { }` — each body is a detached chunk
    /// run by the `RUN_TRY` orchestrator, which returns a control status. The
    /// parent branches on that status: normal falls through; a pending
    /// return/throw re-halts this chunk to propagate it to the enclosing frame;
    /// break/continue jump to the enclosing loop's fixups.
    fn compile_try(
        &mut self,
        b: &mut ChunkBuilder,
        body: &[Stmt],
        catches: &[CatchArm],
        finally: Option<&[Stmt]>,
        line: u32,
    ) -> Result<(), String> {
        let try_chunk = self.compile_detached(body)?;
        let mut cc = Vec::with_capacity(catches.len());
        for c in catches {
            if c.var.as_deref() == Some("this") {
                return Err(self.compile_fatal(line, "Cannot re-assign $this"));
            }
            cc.push(CatchClause {
                classes: c.types.clone(),
                var: c.var.clone(),
                chunk: self.compile_detached(&c.body)?,
            });
        }
        let finally_chunk = match finally {
            Some(f) => Some(self.compile_detached(f)?),
            None => None,
        };
        let id = (self.try_base + self.try_defs.len()) as i64;
        self.try_defs.push(TryDef {
            try_chunk,
            catches: cc,
            finally_chunk,
        });

        // RUN_TRY leaves the control status int on the stack.
        b.emit(Op::LoadInt(id), line);
        b.emit(Op::CallBuiltin(ops::RUN_TRY, 1), line);

        // Dispatch on the status: 1 return, 2 throw, 3 break, 4 continue, 0 normal.
        let j_ret = self.branch_if_status(b, 1, line);
        let j_throw = self.branch_if_status(b, 2, line);
        let j_break = self.branch_if_status(b, 3, line);
        let j_cont = self.branch_if_status(b, 4, line);
        // Normal: discard the status and continue past the construct.
        b.emit(Op::Pop, line);
        let j_end = b.emit(Op::Jump(0), line);

        // Return/throw: the value is already stashed in the host signal; drop the
        // status int and halt this chunk so the enclosing frame propagates it.
        let ret_pos = b.current_pos();
        b.patch_jump(j_ret, ret_pos);
        b.emit(Op::Pop, line);
        b.emit(Op::CallBuiltin(ops::SIG_HALT, 0), line);
        b.emit(Op::Pop, line);

        let throw_pos = b.current_pos();
        b.patch_jump(j_throw, throw_pos);
        b.emit(Op::Pop, line);
        b.emit(Op::CallBuiltin(ops::SIG_HALT, 0), line);
        b.emit(Op::Pop, line);

        // Break/continue: jump to the enclosing loop's fixups (registered here so
        // a `break` inside a try-in-loop reaches the right loop). With no loop
        // present the status is simply discarded (PHP would reject it earlier).
        let break_pos = b.current_pos();
        b.patch_jump(j_break, break_pos);
        b.emit(Op::Pop, line);
        self.emit_break_dispatch(b, true, line);

        let cont_pos = b.current_pos();
        b.patch_jump(j_cont, cont_pos);
        b.emit(Op::Pop, line);
        self.emit_break_dispatch(b, false, line);

        let end = b.current_pos();
        b.patch_jump(j_end, end);
        Ok(())
    }

    /// Route a `break`/`continue` that escaped a `try` body to the loop it was
    /// aimed at.
    ///
    /// The level is only known at run time here (the `break` lives in a separate
    /// chunk), but the number of loops enclosing *this* `try` is known now — so
    /// emit one equality test per enclosing loop and jump into that loop's fixup
    /// list. A level deeper than this chunk's loops belongs to an outer frame:
    /// re-raise the signal with the levels this chunk consumed subtracted off,
    /// and halt so it keeps propagating.
    fn emit_break_dispatch(&mut self, b: &mut ChunkBuilder, is_break: bool, line: u32) {
        let depth = self.loops.len();
        for level in 1..=depth {
            b.emit(Op::CallBuiltin(ops::SIG_LEVEL, 0), line);
            b.emit(Op::LoadInt(level as i64), line);
            b.emit(Op::CallBuiltin(ops::STRICT_EQ, 2), line);
            let j = b.emit(Op::JumpIfTrue(0), line);
            // Fall through to the next level's test; the taken branch is patched
            // below to a jump registered on the matching loop.
            let target = b.current_pos();
            b.patch_jump(j, target + 1);
            let skip = b.emit(Op::Jump(0), line);
            let hit = b.emit(Op::Jump(0), line);
            let idx = depth - level;
            if is_break {
                self.loops[idx].breaks.push(hit);
            } else {
                self.loops[idx].continues.push(hit);
            }
            let after = b.current_pos();
            b.patch_jump(skip, after);
        }
        // Deeper than this chunk's loops: hand the remainder to the outer frame.
        b.emit(Op::CallBuiltin(ops::SIG_LEVEL, 0), line);
        b.emit(Op::LoadInt(depth as i64), line);
        b.emit(Op::Sub, line);
        let sig = if is_break {
            ops::SIG_BREAK
        } else {
            ops::SIG_CONTINUE
        };
        b.emit(Op::CallBuiltin(sig, 1), line);
        b.emit(Op::Pop, line);
    }

    /// Emit `Dup; status === code; if true jump`, returning the pending jump idx.
    /// Leaves the status int on the stack on the fall-through path.
    fn branch_if_status(&mut self, b: &mut ChunkBuilder, code: i64, line: u32) -> usize {
        b.emit(Op::Dup, line);
        b.emit(Op::LoadInt(code), line);
        b.emit(Op::CallBuiltin(ops::STRICT_EQ, 2), line);
        b.emit(Op::CallBuiltin(ops::TRUTHY, 1), line);
        b.emit(Op::JumpIfTrue(0), line)
    }

    // ── expressions ────────────────────────────────────────────────────────

    fn compile_expr(&mut self, b: &mut ChunkBuilder, e: &Expr) -> Result<(), String> {
        // A chain containing a `?->` is lowered whole, so the short-circuit can
        // skip every link that follows it rather than only the one that spelled
        // it. Intercepted here rather than in the two nullsafe arms because the
        // node that OWNS the chain is its outermost link, which is an ordinary
        // `->` or `[…]` whenever the `?->` is not last.
        if chain_has_nullsafe(e) {
            return self.compile_nullsafe_chain(b, e, false);
        }
        match e {
            // `$$x` / `${expr}`: the operand's STRING value is the variable's
            // name, so the lookup is by name rather than through a compiled
            // slot — the name is not known until this runs.
            Expr::VarVar(inner) => {
                self.compile_expr(b, inner)?;
                b.emit(Op::CallBuiltin(ops::GETVAR, 1), self.cur_line);
            }
            Expr::Null => {
                b.emit(Op::LoadUndef, 0);
            }
            // A gap reaches value context only from an array literal that is not
            // a destructuring target — `$a = [1, , 2]`.
            Expr::Hole => {
                return Err(
                    self.compile_fatal(self.cur_line, "Cannot use empty array elements in arrays")
                );
            }
            Expr::Bool(v) => {
                b.emit(if *v { Op::LoadTrue } else { Op::LoadFalse }, 0);
            }
            Expr::Int(n) => {
                b.emit(Op::LoadInt(*n), 0);
            }
            Expr::Float(f) => {
                b.emit(Op::LoadFloat(*f), 0);
            }
            Expr::Str(s) => {
                let idx = b.add_constant(Value::str(s.clone()));
                b.emit(Op::LoadConst(idx), 0);
            }
            Expr::Interp(parts) => self.compile_interp(b, parts)?,
            Expr::Var(name) => self.emit_get_var(b, name),
            // `&` in a VALUE array (`$arr = [&$a]`) makes the element and `$a`
            // one slot; see `compile_array_with_refs`.
            Expr::Array(elems, _) if elems.iter().any(|e| e.by_ref) => {
                self.compile_array_with_refs(b, elems)?
            }
            Expr::Array(elems, _) => {
                // An array literal made only of constants is evaluated by the
                // reference at COMPILE time, so unpacking a scalar inside one is
                // a fatal raised before the script runs, not a catchable Error.
                if let Some(given) = const_array_bad_spread(elems) {
                    return Err(self.compile_fatal(
                        self.cur_line,
                        &format!("Only arrays and Traversables can be unpacked, {given} given"),
                    ));
                }
                // `CallBuiltin`'s operand count is a `u8`, so the pairs go out
                // in chunks: one `MKARRAY` builds the array, and each further
                // chunk extends it through `MKARRAY_ADD`.
                for (chunk, es) in elems.chunks(host::MKARRAY_CHUNK_PAIRS).enumerate() {
                    for e in es {
                        // A `...` element contributes a whole array's entries,
                        // so it takes the spread marker where a key would go
                        // and the compiler emits the operand it unpacks.
                        if let Expr::Spread(inner) = &e.value {
                            let sk = b.add_constant(host::SPREAD_KEY);
                            b.emit(Op::LoadConst(sk), 0);
                            self.compile_expr(b, inner)?;
                            continue;
                        }
                        match &e.key {
                            Some(k) => self.compile_expr(b, k)?,
                            // NOT `LoadUndef`: that is PHP `null`, and a
                            // `null` KEY is the empty string, not the next
                            // integer index. See `host::AUTO_INDEX`.
                            None => {
                                let ai = b.add_constant(host::AUTO_INDEX);
                                b.emit(Op::LoadConst(ai), 0);
                            }
                        }
                        self.compile_expr(b, &e.value)?;
                    }
                    let (op, argc) = if chunk == 0 {
                        (ops::MKARRAY, es.len() * 2)
                    } else {
                        (ops::MKARRAY_ADD, es.len() * 2 + 1)
                    };
                    // Lined, not 0: a literal KEY can diagnose (a float that
                    // loses precision, a null offset) and the report names
                    // this line.
                    b.emit(Op::CallBuiltin(op, argc as u8), self.cur_line);
                }
                // An empty literal still needs its (empty) array.
                if elems.is_empty() {
                    b.emit(Op::CallBuiltin(ops::MKARRAY, 0), self.cur_line);
                }
            }
            Expr::Index(recv, idx) => {
                self.compile_expr(b, recv)?;
                self.compile_expr(b, idx)?;
                b.emit(Op::CallBuiltin(ops::INDEX_GET, 2), self.cur_line);
            }
            Expr::ListElem(recv, idx) => {
                self.compile_expr(b, recv)?;
                self.compile_expr(b, idx)?;
                b.emit(Op::CallBuiltin(ops::LIST_ELEM_GET, 2), self.cur_line);
            }
            Expr::Append(_) => {
                return Err("'[]' append is only valid as an assignment target".into())
            }
            Expr::Unary(op, e) => {
                self.compile_expr(b, e)?;
                match op {
                    UnOp::Neg => {
                        b.emit(Op::Negate, self.cur_line);
                    }
                    // `+$x` is not the identity: PHP applies the same operand
                    // rules as `$x * 1`, so `+"g"` is a `TypeError` and `+"5g"`
                    // warns and yields `5`. Lowering it to that multiplication
                    // gets both without a dedicated opcode — and reports the
                    // `string * int` the reference names, because unary plus
                    // and minus are both multiplications to the engine.
                    UnOp::Pos => {
                        b.emit(Op::LoadInt(1), self.cur_line);
                        b.emit(Op::Mul, self.cur_line);
                    }
                    UnOp::Not => {
                        b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0);
                        b.emit(Op::LogNot, 0);
                    }
                    UnOp::BitNot => {
                        b.emit(Op::CallBuiltin(ops::BITNOT, 1), self.cur_line);
                    }
                }
            }
            Expr::Binary(op, l, r) => self.compile_binary(b, *op, l, r)?,
            Expr::Assign(lhs, op, rhs) => {
                self.check_write_target(lhs)?;
                self.compile_assign(b, lhs, *op, rhs)?
            }
            Expr::IncDec {
                target,
                inc,
                prefix,
            } => {
                self.check_write_target(target)?;
                self.compile_incdec(b, target, *inc, *prefix)?
            }
            Expr::Call(name, args) if has_named(args) => {
                // A call with any `name: value` argument. Push the function name
                // then a `(name, value)` pair per argument for the host to rebind.
                let idx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(idx), 0);
                self.emit_call_name_check(b, name, args.len());
                self.compile_arg_pairs_for(b, name, args, None)?;
                b.emit(
                    Op::CallBuiltin(ops::CALL_NAMED, (args.len() * 2 + 1) as u8),
                    self.cur_line,
                );
                // A user function's parameters are known by name here, so a
                // named argument is written back to the variable it named.
                if let Some(f) = self.byref_user_fns.get(&name.to_ascii_lowercase()) {
                    let targets: Vec<(Expr, usize)> = args
                        .iter()
                        .enumerate()
                        .filter_map(|(i, a)| match a {
                            Expr::NamedArg(n, v) => f
                                .params
                                .iter()
                                .position(|p| p == n)
                                .map(|p| ((**v).clone(), p)),
                            Expr::Spread(_) => None,
                            _ => Some((a.clone(), i)),
                        })
                        .filter(|(_, p)| f.byref.iter().any(|(bp, pn)| bp == p && !pn.is_empty()))
                        .collect();
                    self.emit_byref_writeback_to(b, &targets, true)?;
                } else if let Some(sig) = crate::argsig::sig_of(name) {
                    // A library function's OUT parameters (`preg_match`'s
                    // `$matches`) are found by the name the reference declares
                    // them under, so `matches: $m` is written back like `$m` in
                    // third position.
                    let bound: Vec<(Expr, usize)> = args
                        .iter()
                        .enumerate()
                        .filter_map(|(i, a)| match a {
                            Expr::NamedArg(n, v) => sig
                                .params
                                .iter()
                                .position(|p| p.name == n)
                                .map(|p| ((**v).clone(), p)),
                            Expr::Spread(_) => None,
                            _ => Some((a.clone(), i)),
                        })
                        .collect();
                    let most = bound.iter().map(|(_, p)| p + 1).max().unwrap_or(0);
                    if let Some((positions, _)) = self.byref_positions(name, most) {
                        let targets: Vec<(Expr, usize)> = bound
                            .into_iter()
                            .filter(|(_, p)| positions.contains(p))
                            .collect();
                        self.emit_byref_writeback_to(b, &targets, true)?;
                    }
                }
            }
            Expr::Call(name, args) => {
                let has_spread = args.iter().any(|a| matches!(a, Expr::Spread(_)));
                // The by-reference array mutators take their array by variable
                // name so the host can rewrite (and auto-vivify) it in place. A
                // spread among the arguments falls through to the normal dispatch.
                // Every argument slot these take by reference is the FIRST, and
                // the diagnostic for one that cannot be bound is the same as for
                // any other by-reference builtin.
                let mut_diag = args.first().map(byref_arg_class);
                let mutator_target = match (has_spread, array_mutator_subop(name), args.first()) {
                    (false, Some(sub), Some(Expr::Var(vname))) => Some((sub, vname.clone())),
                    // Anything that is not a plain variable reaches the mutator
                    // through a temporary holding the array HANDLE itself — a
                    // plain `SETVAR`, which does not copy — so a mutation through
                    // it still lands on the original.
                    //
                    // For `$this->stack` that is the point: the property must
                    // see the change. For a value that has no home, such as a
                    // call result, the temporary is where the reference writes
                    // too, and the mutation is simply discarded with it.
                    (false, Some(sub), Some(root)) => {
                        let tmp = self.tmp_name("mut");
                        let root = root.clone();
                        // `emit_set_var` already discards the assignment's own
                        // result, so nothing is left to pop here. Popping again
                        // took the operand BELOW it — the enclosing call's
                        // function name — which is why `var_dump(array_pop(
                        // $o->prop))` used to die as `Call to undefined
                        // function ()`.
                        self.emit_set_var(b, &tmp, |c, b| c.compile_expr(b, &root))?;
                        Some((sub, tmp))
                    }
                    _ => None,
                };
                if let Some((sub, vname)) = mutator_target {
                    if let Some(class) = mut_diag {
                        self.emit_byref_arg_diag(b, name, 1, "array", class);
                    }
                    let nidx = b.add_constant(Value::str(vname.clone()));
                    b.emit(Op::LoadConst(nidx), 0);
                    b.emit(Op::LoadInt(sub), 0);
                    for a in &args[1..] {
                        self.compile_expr(b, a)?;
                    }
                    // argc = name + subop + the remaining value arguments.
                    b.emit(
                        Op::CallBuiltin(ops::ARR_MUT, (args.len() + 1) as u8),
                        self.cur_line,
                    );
                } else if has_spread {
                    // Any `...$arr` argument switches to the spread dispatch: each
                    // argument is pushed as a `(is_spread, value)` pair so the host
                    // can flatten spread arrays into the positional argument list.
                    let idx = b.add_constant(Value::str(name.clone()));
                    b.emit(Op::LoadConst(idx), 0);
                    // An argument written BEFORE the first spread still lands in a
                    // known position, so a by-reference parameter there is judged
                    // exactly as in a call with no spread at all: `f(1, ...[2, 3])`
                    // on `function f(&$a)` is refused. From the spread on, how many
                    // arguments it contributes is a run-time fact, so no position
                    // after it can be judged here.
                    let (shown, diag) = self.byref_slots_for(name, args.len());
                    let mut seen_spread = false;
                    for (i, a) in args.iter().enumerate() {
                        match a {
                            Expr::Spread(inner) => {
                                seen_spread = true;
                                b.emit(Op::LoadTrue, 0);
                                self.compile_expr(b, inner)?;
                            }
                            _ => {
                                b.emit(Op::LoadFalse, 0);
                                self.compile_expr(b, a)?;
                                if !seen_spread {
                                    if let Some((_, argno, param)) =
                                        diag.iter().find(|(p, ..)| *p == i).cloned()
                                    {
                                        self.emit_byref_arg_diag(
                                            b,
                                            &shown,
                                            argno,
                                            &param,
                                            byref_arg_class(a),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    b.emit(
                        Op::CallBuiltin(ops::CALL_SPREAD, (args.len() * 2 + 1) as u8),
                        self.cur_line,
                    );
                    // The by-reference write-back reaches the positions BEFORE the
                    // first spread, for the same reason the refusal above does:
                    // those are the ones whose argument number is known here.
                    // Without it `f($q, ...[2, 3])` on `function f(&$a)` left $q
                    // at its old value while `f($q)` updated it.
                    let first_spread = args
                        .iter()
                        .position(|a| matches!(a, Expr::Spread(_)))
                        .unwrap_or(args.len());
                    if let Some((positions, guarded)) = self.byref_positions(name, args.len()) {
                        let known: Vec<usize> = positions
                            .into_iter()
                            .filter(|p| *p < first_spread)
                            .collect();
                        self.emit_byref_writeback(b, args, &known, guarded)?;
                    }
                } else if is_direct_minmax2(name, args) {
                    // A literal two-argument `min`/`max`. The reference compiles
                    // exactly this shape to its FRAMELESS implementation, which
                    // answers differently from the variadic one when an operand
                    // is a NaN, so the shape is recorded in the opcode rather
                    // than lost in a uniform call.
                    let idx = b.add_constant(Value::str(name.clone()));
                    b.emit(Op::LoadConst(idx), 0);
                    for a in args {
                        self.compile_expr(b, a)?;
                    }
                    b.emit(Op::CallBuiltin(ops::MINMAX_FLF2, 3), self.cur_line);
                } else {
                    let idx = b.add_constant(Value::str(name.clone()));
                    b.emit(Op::LoadConst(idx), 0);
                    self.emit_call_name_check(b, name, args.len());
                    let byref = self.byref_positions(name, args.len());
                    let diag = byref_diag_slots(name, args.len());
                    // A USER function declares its own by-reference parameters,
                    // and the reference refuses an argument that cannot supply
                    // one there exactly as it does for a library function.
                    let user = self.byref_user_slots(name, args.len());
                    for (i, a) in args.iter().enumerate() {
                        // An argument in a by-reference position is an output
                        // location, not a value the call reads, so an unset one is
                        // not a mistake and PHP raises no diagnostic for it —
                        // `preg_match($re, $s, $m)` with a fresh `$m` is the norm.
                        // The same holds for a library function that takes its
                        // array by reference to mutate it: `sort($unset)` binds
                        // the variable to null silently, and the refusal is the
                        // `TypeError` for that null.
                        let in_diag = diag.iter().any(|(p, ..)| *p == i);
                        match &byref {
                            Some((p, _)) if p.contains(&i) => self.compile_quiet(b, a)?,
                            _ if in_diag => self.compile_quiet(b, a)?,
                            _ => self.compile_expr(b, a)?,
                        }
                        // Whether that position can actually be WRITTEN back to is
                        // a separate question from whether reading it is quiet,
                        // and it is settled here, between this argument and the
                        // next — the reference never evaluates the arguments after
                        // one it is about to reject.
                        if let Some(&(_, argno, param)) = diag.iter().find(|(p, ..)| *p == i) {
                            self.emit_byref_arg_diag(b, name, argno, param, byref_arg_class(a));
                        } else if let Some((spelled, argno, param)) =
                            user.as_ref().and_then(|(s, slots)| {
                                slots
                                    .iter()
                                    .find(|(p, ..)| *p == i)
                                    .map(|(_, n, pn)| (s.clone(), *n, pn.clone()))
                            })
                        {
                            self.emit_byref_arg_diag(
                                b,
                                &spelled,
                                argno,
                                &param,
                                byref_arg_class(a),
                            );
                        }
                    }
                    b.emit(
                        Op::CallBuiltin(ops::CALL, (args.len() + 1) as u8),
                        self.cur_line,
                    );
                    // By-reference parameters: write the callee's final values back
                    // to the caller's argument variables (leaving the call result).
                    // The callee is named here, so which positions those are is a
                    // compile-time fact and no run-time guard is needed.
                    if let Some((positions, guarded)) = byref {
                        self.emit_byref_writeback(b, args, &positions, guarded)?;
                    }
                }
            }
            Expr::Spread(_) => {
                return Err("'...' argument unpacking is only valid in a function call".into())
            }
            Expr::Fcc { callable, instance } => self.compile_fcc(b, callable, *instance)?,
            Expr::CallValue(callee, args) if needs_arg_pairs(args) => {
                self.compile_expr(b, callee)?;
                self.emit_callee_check(b, ops::CALLVALUE_CHECK, args.len());
                self.compile_arg_pairs(b, args)?;
                b.emit(
                    Op::CallBuiltin(ops::CALLVALUE_NAMED, (args.len() * 2 + 1) as u8),
                    self.cur_line,
                );
            }
            Expr::CallValue(callee, args) => {
                self.compile_expr(b, callee)?;
                self.emit_callee_check(b, ops::CALLVALUE_CHECK, args.len());
                for a in args {
                    self.compile_expr(b, a)?;
                }
                b.emit(
                    Op::CallBuiltin(ops::CALL_VALUE, (args.len() + 1) as u8),
                    self.cur_line,
                );
                let all = (0..args.len()).collect::<Vec<_>>();
                self.emit_byref_writeback(b, args, &all, true)?;
            }
            Expr::Closure {
                params,
                uses,
                body,
                ret,
                is_static,
                line,
            } => {
                self.check_closure_uses(params, uses, *line)?;
                let saved_rule =
                    std::mem::replace(&mut self.ret_rule, ret_rule(ret.as_ref(), body));
                self.compile_closure(b, params, uses, body, ret.as_ref(), *is_static, *line)?;
                self.ret_rule = saved_rule;
            }
            Expr::ArrowFn {
                params,
                body,
                ret: ret_ty,
                is_static,
                line,
            } => {
                // An arrow fn desugars to a closure whose single-statement body
                // returns the expression; it captures every free variable of the
                // body (minus its own parameters) by value. The synthesized
                // statement carries the `fn` keyword's line, so a diagnostic raised
                // by the body names it rather than the enclosing statement.
                let ret = vec![Stmt {
                    line: *line,
                    kind: StmtKind::Return(Some((**body).clone())),
                }];
                let mut captures = Vec::new();
                collect_free_vars(body, &mut captures);
                captures.retain(|n| !params.iter().any(|p| p.name == *n));
                // A `static fn` is never given the enclosing `$this`.
                if *is_static {
                    captures.retain(|n| n != "this");
                }
                // An arrow function has no `use` clause, so every capture is by
                // value — PHP has no by-reference form of it.
                let captures: Vec<Capture> = captures
                    .into_iter()
                    .map(|name| Capture {
                        name,
                        by_ref: false,
                    })
                    .collect();
                // The body IS a returned expression, which a `void` arrow function
                // refuses; a `never` one is exempt (`fn(): never => throw $e`).
                let rule = ret_rule(ret_ty.as_ref(), &ret).filter(|r| *r == "void");
                let saved_rule = std::mem::replace(&mut self.ret_rule, rule);
                self.compile_closure(
                    b,
                    params,
                    &captures,
                    &ret,
                    ret_ty.as_ref(),
                    *is_static,
                    *line,
                )?;
                self.ret_rule = saved_rule;
            }
            // The declaration is compiled here, once, and the expression becomes
            // an ordinary `new` on the name it was given — so re-evaluating it
            // (in a loop, say) reuses the single class, as PHP does.
            Expr::NewAnon { decl, args, line } => {
                let name = self.anon_class_name(decl, *line);
                let named = ClassDecl {
                    name: name.clone(),
                    ..(**decl).clone()
                };
                // Lowering the body walks its statements, which leaves
                // `cur_line` on the class's last member. The `new` belongs to
                // the `class` keyword's own line — an exception constructed
                // there reports that line, not the enclosing statement's — and
                // the rest of the statement belongs to where it started.
                let site = self.cur_line;
                let before = self.classes.len();
                self.compile_class(b, &named)?;
                if self.classes.len() > before {
                    self.check_finals(b, &named, false, *line)?;
                }
                self.cur_line = *line;
                self.compile_expr(b, &Expr::New(name, args.clone()))?;
                self.cur_line = site;
            }
            // The instance is allocated before the arguments are evaluated (see
            // `ops::NEW_ALLOC`), and the constructor runs over it after.
            Expr::New(class, args) if needs_arg_pairs(args) => {
                self.emit_class_name(b, class)?;
                self.emit_callee_check(b, ops::CALL_CLASS_CHECK, args.len());
                b.emit(Op::CallBuiltin(ops::NEW_ALLOC, 1), self.cur_line);
                self.compile_arg_pairs(b, args)?;
                b.emit(
                    Op::CallBuiltin(ops::NEW_INIT_NAMED, (args.len() * 2 + 1) as u8),
                    self.cur_line,
                );
            }
            // The class operand is evaluated first and resolved to a name the
            // way every dynamic `::` is (`DYN_CLASS`: a string as written, an
            // object's own class, anything else an `Error`), and then the
            // ordinary `new` sequence runs on that name.
            Expr::NewDyn(class, args) => {
                self.compile_expr(b, class)?;
                b.emit(Op::CallBuiltin(ops::DYN_CLASS, 1), self.cur_line);
                self.emit_callee_check(b, ops::CALL_CLASS_CHECK, args.len());
                b.emit(Op::CallBuiltin(ops::NEW_ALLOC, 1), self.cur_line);
                if needs_arg_pairs(args) {
                    self.compile_arg_pairs(b, args)?;
                    b.emit(
                        Op::CallBuiltin(ops::NEW_INIT_NAMED, (args.len() * 2 + 1) as u8),
                        self.cur_line,
                    );
                } else {
                    for a in args {
                        self.compile_expr(b, a)?;
                    }
                    b.emit(
                        Op::CallBuiltin(ops::NEW_INIT, (args.len() + 1) as u8),
                        self.cur_line,
                    );
                }
            }
            Expr::New(class, args) => {
                self.emit_class_name(b, class)?;
                self.emit_callee_check(b, ops::CALL_CLASS_CHECK, args.len());
                b.emit(Op::CallBuiltin(ops::NEW_ALLOC, 1), self.cur_line);
                for a in args {
                    self.compile_expr(b, a)?;
                }
                b.emit(
                    Op::CallBuiltin(ops::NEW_INIT, (args.len() + 1) as u8),
                    self.cur_line,
                );
            }
            Expr::PropGet(recv, name) => {
                self.compile_expr(b, recv)?;
                self.emit_member(b, name, 0)?;
                b.emit(Op::CallBuiltin(ops::PROP_GET, 2), self.cur_line);
            }
            Expr::MethodCall(recv, name, args) if needs_arg_pairs(args) => {
                // As for the plain form below: the receiver is held for the
                // by-reference judgement of the positional arguments.
                let t = self.tmp_name("mrecv");
                self.emit_set_var(b, &t, |c, b| c.compile_expr(b, recv))?;
                self.emit_get_var(b, &t);
                let name = &self.method_member(b, name, args.len())?;
                self.emit_mcall_recv_check(b, name, args.len())?;
                self.emit_member(b, name, 0)?;
                self.compile_arg_pairs_m(b, args, Some((&t, name)))?;
                b.emit(
                    Op::CallBuiltin(ops::MCALL_NAMED, (args.len() * 2 + 2) as u8),
                    self.cur_line,
                );
                self.emit_byref_writeback(b, args, &leading_positional(args), true)?;
            }
            Expr::MethodCall(recv, name, args) => {
                // An argument with no location of its own is judged against
                // the method's by-reference parameters as it is sent, which
                // needs the receiver again: it is held in a temporary.
                let recv_t = match args.iter().any(|a| byref_arg_class(a) != ByRefArg::Lvalue) {
                    true => {
                        let t = self.tmp_name("mrecv");
                        self.emit_set_var(b, &t, |c, b| c.compile_expr(b, recv))?;
                        self.emit_get_var(b, &t);
                        Some(t)
                    }
                    false => {
                        self.compile_expr(b, recv)?;
                        None
                    }
                };
                let name = &self.method_member(b, name, args.len())?;
                self.emit_mcall_recv_check(b, name, args.len())?;
                self.emit_member(b, name, 0)?;
                for (i, a) in args.iter().enumerate() {
                    self.compile_expr(b, a)?;
                    if let Some(t) = &recv_t {
                        self.emit_byref_arg_diag_m(
                            b,
                            |c, b| {
                                c.emit_get_var(b, t);
                                Ok(())
                            },
                            name,
                            i,
                            a,
                        )?;
                    }
                }
                b.emit(
                    Op::CallBuiltin(ops::MCALL, (args.len() + 2) as u8),
                    self.cur_line,
                );
                let all = (0..args.len()).collect::<Vec<_>>();
                self.emit_byref_writeback(b, args, &all, true)?;
            }
            // Reached only if the interception at the top of this function is
            // ever removed: a nullsafe link is ALWAYS lowered as part of its
            // whole chain, never on its own.
            Expr::NullsafePropGet(..) | Expr::NullsafeMethodCall(..) => {
                self.compile_nullsafe_chain(b, e, false)?;
            }
            Expr::NamedArg(_, inner) => {
                // A named argument outside a handled call site: compile its value
                // (the name is only meaningful in an argument list).
                self.compile_expr(b, inner)?;
            }
            Expr::StaticGet(class, name) => {
                // `Class::class` / `self::class` yields the resolved class-name
                // string, not a class constant — and `static::class` the one the
                // running call was made on.
                if name.eq_ignore_ascii_case("class") {
                    match class {
                        // `$expr::class` is stricter than every other `::`: it
                        // answers for an object and rejects a string, so it
                        // cannot share `DYN_CLASS`.
                        ClassRef::Expr(e) => {
                            self.compile_expr(b, e)?;
                            b.emit(Op::CallBuiltin(ops::DYN_CLASS_CONST, 1), self.cur_line);
                        }
                        ClassRef::Name(n) => self.emit_class_name_as(b, n, true)?,
                    }
                } else {
                    self.emit_class_ref(b, class)?;
                    let nidx = b.add_constant(Value::str(name.clone()));
                    b.emit(Op::LoadConst(nidx), 0);
                    b.emit(Op::CallBuiltin(ops::SCONST, 2), self.cur_line);
                }
            }
            Expr::StaticProp(class, name) => {
                self.emit_class_ref(b, class)?;
                let nidx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(nidx), 0);
                b.emit(Op::CallBuiltin(ops::SPROP_GET, 2), 0);
            }
            Expr::StaticCall(class, name, args) if needs_arg_pairs(args) => {
                self.emit_lsb_forward(b, class);
                self.emit_class_ref(b, class)?;
                let nidx = b.add_constant(Value::str(name.clone()));
                self.emit_scall_callee_check(b, nidx, args.len());
                b.emit(Op::LoadConst(nidx), 0);
                self.compile_arg_pairs(b, args)?;
                b.emit(
                    Op::CallBuiltin(ops::SCALL_NAMED, (args.len() * 2 + 2) as u8),
                    self.cur_line,
                );
                self.emit_byref_writeback(b, args, &leading_positional(args), true)?;
            }
            Expr::StaticCall(class, name, args) => {
                self.emit_lsb_forward(b, class);
                self.emit_class_ref(b, class)?;
                let nidx = b.add_constant(Value::str(name.clone()));
                self.emit_scall_callee_check(b, nidx, args.len());
                b.emit(Op::LoadConst(nidx), 0);
                for (i, a) in args.iter().enumerate() {
                    self.compile_expr(b, a)?;
                    // Re-pushing the class has no side effect only for a NAME;
                    // `$expr::m()` is left unjudged.
                    if matches!(class, ClassRef::Name(_)) {
                        let member = Member::Name(name.clone());
                        self.emit_byref_arg_diag_m(
                            b,
                            |c, b| c.emit_class_ref(b, class),
                            &member,
                            i,
                            a,
                        )?;
                    }
                }
                b.emit(
                    Op::CallBuiltin(ops::SCALL, (args.len() + 2) as u8),
                    self.cur_line,
                );
                let all = (0..args.len()).collect::<Vec<_>>();
                self.emit_byref_writeback(b, args, &all, true)?;
            }
            Expr::Ternary(c, t, f) => {
                self.compile_truthy(b, c)?;
                let jf = b.emit(Op::JumpIfFalse(0), 0);
                self.compile_expr(b, t)?;
                let jend = b.emit(Op::Jump(0), 0);
                let els = b.current_pos();
                b.patch_jump(jf, els);
                self.compile_expr(b, f)?;
                let end = b.current_pos();
                b.patch_jump(jend, end);
            }
            Expr::Elvis(a, els) => {
                // `a ?: b` — evaluate `a` once; keep it if truthy, else use `b`.
                self.compile_expr(b, a)?; // [a]
                b.emit(Op::Dup, 0); // [a, a]
                b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0); // [a, bool]
                let keep = b.emit(Op::JumpIfTrue(0), 0); // truthy → keep a, leaving [a]
                b.emit(Op::Pop, 0); // discard a
                self.compile_expr(b, els)?; // [b]
                let jend = b.emit(Op::Jump(0), 0);
                let keep_pos = b.current_pos();
                b.patch_jump(keep, keep_pos);
                let end = b.current_pos();
                b.patch_jump(jend, end);
            }
            Expr::Quiet(inner) => self.compile_quiet(b, inner)?,
            // `@expr` — the operand compiles NORMALLY, wrapped in a run-time
            // suppression region. Not `compile_quiet`: `@` drops diagnostics, it
            // does not change what the operand means, and the region is what
            // catches the diagnostics raised inside the functions it calls
            // (`@preg_match('/[a', $s)`, `@range('ab', 'c')`) — those are raised
            // from Rust and have no opcode to quieten.
            Expr::Suppress(inner) => {
                b.emit(Op::CallBuiltin(ops::SUPPRESS_PUSH, 0), self.cur_line);
                b.emit(Op::Pop, self.cur_line);
                self.compile_expr(b, inner)?;
                b.emit(Op::CallBuiltin(ops::SUPPRESS_POP, 1), self.cur_line);
            }
            // One `isset()` argument. A property target gets its own opcode
            // because `isset` asks `__isset` and NOTHING else: a class whose
            // `__isset` returns true is set even if `__get` would answer null, so
            // the answer cannot be recovered from a value the way it can for a
            // variable or an array element.
            Expr::IssetOf(inner) => match inner.as_ref() {
                Expr::PropGet(recv, name) => {
                    self.compile_quiet(b, recv)?;
                    self.emit_member(b, name, self.cur_line)?;
                    b.emit(Op::CallBuiltin(ops::PROP_ISSET, 2), self.cur_line);
                }
                // An index target gets its own opcode for the same reason a
                // property does: `isset($o[k])` on an `ArrayAccess` asks
                // `offsetExists` and NOTHING else, so the answer cannot be
                // recovered by comparing a read value against null.
                Expr::Index(recv, key) => {
                    self.compile_quiet(b, recv)?;
                    self.compile_expr(b, key)?;
                    b.emit(Op::CallBuiltin(ops::INDEX_ISSET, 2), self.cur_line);
                }
                other => {
                    // Everything else: set means "reads as something other than
                    // null", which an isset-mode read answers directly.
                    self.compile_quiet(b, other)?;
                    let idx = b.add_constant(Value::Undef);
                    b.emit(Op::LoadConst(idx), self.cur_line);
                    b.emit(Op::CallBuiltin(ops::STRICT_NE, 2), self.cur_line);
                }
            },
            // The `empty()` argument. Only a property target differs from an
            // ordinary isset-mode read — see `ops::PROP_GET_EMPTY`.
            Expr::EmptyOf(inner) => match inner.as_ref() {
                Expr::PropGet(recv, name) => {
                    self.compile_quiet(b, recv)?;
                    self.emit_member(b, name, self.cur_line)?;
                    b.emit(Op::CallBuiltin(ops::PROP_GET_EMPTY, 2), self.cur_line);
                }
                // An index target reads like `??` does, but an offset the
                // reference refuses is worded for `empty()` — see
                // `ops::INDEX_GET_EMPTY`.
                Expr::Index(recv, key) => {
                    self.compile_quiet(b, recv)?;
                    self.compile_expr(b, key)?;
                    b.emit(Op::CallBuiltin(ops::INDEX_GET_EMPTY, 2), self.cur_line);
                }
                other => self.compile_quiet(b, other)?,
            },
            Expr::Coalesce(a, els) => {
                // `a ?? b` — use `b` only when `a` is null (=== null). The left
                // operand is an isset-mode read: `$a["k"] ?? $d` is exactly the
                // question `isset($a["k"])` asks, so a MISSING key raises no
                // diagnostic. A lossy OFFSET still does — see `INDEX_GET_Q`.
                self.compile_quiet(b, a)?; // [a]
                b.emit(Op::Dup, 0); // [a, a]
                b.emit(Op::LoadUndef, 0); // [a, a, null]
                b.emit(Op::CallBuiltin(ops::STRICT_EQ, 2), 0); // [a, a===null]
                b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0); // [a, bool]
                let use_b = b.emit(Op::JumpIfTrue(0), 0); // a is null → use b
                let jend = b.emit(Op::Jump(0), 0); // a not null → keep a, leaving [a]
                let use_b_pos = b.current_pos();
                b.patch_jump(use_b, use_b_pos);
                b.emit(Op::Pop, 0); // discard a
                self.compile_expr(b, els)?; // [b]
                let end = b.current_pos();
                b.patch_jump(jend, end);
            }
            Expr::Match { subj, arms } => self.compile_match(b, subj, arms)?,
            Expr::Clone(inner) => {
                self.compile_expr(b, inner)?;
                b.emit(Op::CallBuiltin(ops::CLONE, 1), self.cur_line);
            }
            Expr::Throw(inner) => {
                // Evaluate the exception object, record it as pending, and unwind
                // the current chunk. As an expression it produces no value, but
                // the THROW builtin leaves an Undef the surrounding context pops.
                self.compile_expr(b, inner)?;
                b.emit(Op::CallBuiltin(ops::THROW, 1), self.cur_line);
            }
            Expr::ConstFetch(name) => {
                let idx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(idx), 0);
                // Sited: an undefined constant throws from here, and the `Error`
                // reports this op's line.
                b.emit(Op::CallBuiltin(ops::CONST_FETCH, 1), self.cur_line);
            }
            // A magic constant the parse could not settle: the affixes it carries
            // are compile-time literals, and only the piece between them is read
            // from the host.
            Expr::Magic(m) => {
                let (op, prefix, suffix) = match m {
                    MagicConst::File { prefix, suffix } => (ops::MAGIC_FILE, prefix, suffix),
                    MagicConst::Class { prefix, suffix } => (ops::MAGIC_CLASS, prefix, suffix),
                    MagicConst::Dir => {
                        b.emit(Op::CallBuiltin(ops::MAGIC_DIR, 0), 0);
                        return Ok(());
                    }
                };
                let p = b.add_constant(Value::str(prefix.clone()));
                b.emit(Op::LoadConst(p), 0);
                let s = b.add_constant(Value::str(suffix.clone()));
                b.emit(Op::LoadConst(s), 0);
                b.emit(Op::CallBuiltin(op, 2), 0);
            }
            Expr::Unset(targets) => {
                for t in targets {
                    self.compile_unset_target(b, t)?;
                }
                // `unset(...)` is a statement construct; it evaluates to null.
                b.emit(Op::LoadUndef, 0);
            }
            Expr::InstanceOf(e, class) => {
                self.compile_expr(b, e)?;
                self.emit_class_name(b, class)?;
                b.emit(Op::CallBuiltin(ops::INSTANCEOF, 2), 0);
            }
            // `ZEND_FETCH_CLASS` on the right operand runs after the left is
            // evaluated, and refuses a value that is neither object nor string.
            Expr::InstanceOfDyn(e, class) => {
                self.compile_expr(b, e)?;
                self.compile_expr(b, class)?;
                b.emit(Op::CallBuiltin(ops::DYN_CLASS, 1), self.cur_line);
                b.emit(Op::CallBuiltin(ops::INSTANCEOF, 2), 0);
            }
            Expr::RefAssign(lhs, rhs) => self.compile_ref_assign(b, lhs, rhs)?,
            Expr::Yield { key, value } => {
                // Leave the yielded value (and, for the keyed form, the key) on the
                // stack, then suspend the running generator. The YIELD builtin
                // returns the value the next `->send($x)`/`->next()` supplies, so
                // `$x = yield ...` sees it.
                match value {
                    Some(v) => self.compile_expr(b, v)?,
                    None => {
                        b.emit(Op::LoadUndef, 0);
                    }
                }
                match key {
                    Some(k) => {
                        self.compile_expr(b, k)?;
                        b.emit(Op::CallBuiltin(ops::YIELD_KV, 2), self.cur_line);
                    }
                    None => {
                        b.emit(Op::CallBuiltin(ops::YIELD, 1), self.cur_line);
                    }
                }
            }
            // `print E` is `echo E` with the int `1` left behind: the same
            // opcode, then its `Undef` swapped for the value PHP documents.
            Expr::Print(operand) => {
                let line = self.cur_line;
                self.compile_expr(b, operand)?;
                b.emit(Op::CallBuiltin(ops::ECHO, 1), line);
                b.emit(Op::Pop, line);
                let idx = b.add_constant(Value::int(1));
                b.emit(Op::LoadConst(idx), line);
            }
            Expr::YieldFrom(src) => {
                self.compile_expr(b, src)?;
                b.emit(Op::CallBuiltin(ops::YIELD_FROM, 1), self.cur_line);
            }
            // The file or code is compiled when the construct RUNS, and runs in
            // this frame — see `host::run_include`.
            Expr::Include(kind, path) => {
                let line = self.cur_line;
                self.compile_expr(b, path)?;
                b.emit(Op::LoadInt(kind.code()), line);
                b.emit(Op::CallBuiltin(ops::INCLUDE, 2), line);
            }
            Expr::Eval(code) => {
                let line = self.cur_line;
                self.compile_expr(b, code)?;
                b.emit(Op::CallBuiltin(ops::EVAL, 1), line);
            }
        }
        Ok(())
    }

    /// Compile a read in PHP's "isset mode" — the operand of `isset()`,
    /// `empty()`, `@`, or the left side of `??`. A missing variable, element or
    /// property is the question being asked, so the read raises no diagnostic.
    ///
    /// Only the chain of reads itself is quietened: an index expression, a method
    /// argument or any other nested subexpression is compiled normally, so a
    /// function call inside a key still reports its own diagnostics — which is
    /// what PHP's compile-time `BP_VAR_IS` fetch mode does.
    fn compile_quiet(&mut self, b: &mut ChunkBuilder, e: &Expr) -> Result<(), String> {
        // Same whole-chain lowering as the loud path, in `BP_VAR_IS` mode.
        if chain_has_nullsafe(e) {
            return self.compile_nullsafe_chain(b, e, true);
        }
        let line = self.cur_line;
        match e {
            Expr::Quiet(inner) => self.compile_quiet(b, inner)?,
            Expr::Var(name) => {
                // A promoted local is written before it is ever read, so the
                // quiet read and the loud one cannot differ for it.
                if let Some(&i) = self.fslots.get(name) {
                    b.emit(Op::GetSlot(i), line);
                } else if let Some(&i) = self.slots.get(name) {
                    b.emit(Op::LoadInt(i as i64), line);
                    b.emit(Op::CallBuiltin(ops::GETSLOT_Q, 1), line);
                } else {
                    let idx = b.add_constant(Value::str(name.clone()));
                    b.emit(Op::LoadConst(idx), line);
                    b.emit(Op::CallBuiltin(ops::GETVAR_Q, 1), line);
                }
            }
            // The NAME is still computed the ordinary way — it is the read of
            // the variable it names that has to stay quiet, so `isset($$x)` on
            // an unbound name answers false instead of warning.
            Expr::VarVar(inner) => {
                self.compile_expr(b, inner)?;
                b.emit(Op::CallBuiltin(ops::GETVAR_Q, 1), line);
            }
            Expr::Index(recv, idx) => {
                self.compile_quiet(b, recv)?;
                self.compile_expr(b, idx)?;
                b.emit(Op::CallBuiltin(ops::INDEX_GET_Q, 2), line);
            }
            Expr::PropGet(recv, name) => {
                self.compile_quiet(b, recv)?;
                self.emit_member(b, name, line)?;
                b.emit(Op::CallBuiltin(ops::PROP_GET_Q, 2), line);
            }
            Expr::StaticProp(class, name) => {
                self.emit_class_ref(b, class)?;
                let nidx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(nidx), line);
                b.emit(Op::CallBuiltin(ops::SPROP_GET_Q, 2), line);
            }
            other => self.compile_expr(b, other)?,
        }
        Ok(())
    }

    /// `lhs = &rhs` — bind the left-hand side to the storage cell the right-hand
    /// side denotes. Lowered in two halves (see `ops::REF_SLOT_VAR`): the
    /// right-hand side is resolved to a reference cell, then the left-hand side is
    /// pointed at it, so every combination of variable / array element / object
    /// property on either side is covered by composing the two.
    fn compile_ref_assign(
        &mut self,
        b: &mut ChunkBuilder,
        lhs: &Expr,
        rhs: &Expr,
    ) -> Result<(), String> {
        if matches!(lhs, Expr::Var(n) if n == "this") {
            return Err(self.compile_fatal(self.cur_line, "Cannot re-assign $this"));
        }
        self.check_write_target(lhs)?;
        if chain_has_nullsafe(rhs) {
            return Err(
                self.compile_fatal(self.cur_line, "Cannot take reference of a nullsafe chain")
            );
        }
        self.check_write_base(rhs)?;
        // `$a = &$b` between two plain variables keeps its compact lowering.
        if let (Expr::Var(t), Expr::Var(s)) = (lhs, rhs) {
            let ti = b.add_constant(Value::str(t.clone()));
            b.emit(Op::LoadConst(ti), 0);
            let si = b.add_constant(Value::str(s.clone()));
            b.emit(Op::LoadConst(si), 0);
            b.emit(Op::CallBuiltin(ops::REF_BIND, 2), 0);
            return Ok(());
        }
        match lhs {
            Expr::Var(name) => {
                let ni = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(ni), 0);
                self.compile_ref_slot(b, rhs)?;
                b.emit(Op::CallBuiltin(ops::REF_TO_VAR, 2), 0);
            }
            Expr::Index(..) | Expr::Append(..) => {
                let (root, segs) = Self::flatten_segments(lhs)?;
                let name = self.ref_root_name(b, root)?;
                let append = matches!(segs.last(), Some(LvSeg::Append));
                let key_segs = if append {
                    &segs[..segs.len() - 1]
                } else {
                    &segs[..]
                };
                let ni = b.add_constant(Value::str(name));
                b.emit(Op::LoadConst(ni), 0);
                for s in key_segs {
                    match s {
                        LvSeg::Key(k) => self.compile_expr(b, k)?,
                        LvSeg::Append => {
                            return Err("`[]` may appear only as the last segment of a \
                                        reference assignment"
                                .into())
                        }
                    }
                }
                self.compile_ref_slot(b, rhs)?;
                let op = if append {
                    ops::REF_TO_APPEND
                } else {
                    ops::REF_TO_ELEM
                };
                b.emit(Op::CallBuiltin(op, (key_segs.len() + 2) as u8), 0);
            }
            Expr::PropGet(recv, prop) => {
                self.compile_expr(b, recv)?;
                self.emit_member(b, prop, 0)?;
                self.compile_ref_slot(b, rhs)?;
                b.emit(Op::CallBuiltin(ops::REF_TO_PROP, 3), 0);
            }
            _ => {
                return Err(
                    "reference `= &` assigns only to a variable, an array element or \
                     an object property"
                        .into(),
                )
            }
        }
        Ok(())
    }

    /// Push the reference cell (an `Int` slot) that a `&` operand denotes,
    /// promoting the variable / element / property into a reference if it is not
    /// one already.
    fn compile_ref_slot(&mut self, b: &mut ChunkBuilder, src: &Expr) -> Result<(), String> {
        match src {
            Expr::Var(name) => {
                let ni = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(ni), 0);
                b.emit(Op::CallBuiltin(ops::REF_SLOT_VAR, 1), 0);
            }
            Expr::Index(..) => {
                let (root, segs) = Self::flatten_segments(src)?;
                let name = self.ref_root_name(b, root)?;
                let ni = b.add_constant(Value::str(name));
                b.emit(Op::LoadConst(ni), 0);
                for s in &segs {
                    match s {
                        LvSeg::Key(k) => self.compile_expr(b, k)?,
                        // `&$a[]` has no element to alias — PHP rejects it too.
                        LvSeg::Append => return Err("cannot take a reference to `$a[]`".into()),
                    }
                }
                b.emit(
                    Op::CallBuiltin(ops::REF_SLOT_ELEM, (segs.len() + 1) as u8),
                    0,
                );
            }
            Expr::PropGet(recv, prop) => {
                self.compile_expr(b, recv)?;
                self.emit_member(b, prop, 0)?;
                b.emit(Op::CallBuiltin(ops::REF_SLOT_PROP, 2), 0);
            }
            // `$r = &f()` / `&$o->m()` — the cell a `function &f()` published on
            // its way out. A callee that returned by value has none, and the
            // binding falls back to a detached cell holding the result.
            Expr::Call(..) | Expr::MethodCall(..) | Expr::StaticCall(..) | Expr::CallValue(..) => {
                self.compile_expr(b, src)?;
                b.emit(Op::CallBuiltin(ops::REF_SLOT_RET, 1), 0);
            }
            _ => {
                return Err(
                    "`&` takes a reference to a variable, an array element or an \
                     object property"
                        .into(),
                )
            }
        }
        Ok(())
    }

    /// The scope-variable name a reference path is rooted at. A path rooted at an
    /// object property (`&$this->items[0]`) is re-rooted on a temporary holding
    /// the property's array handle — a plain `SETVAR`, which does not copy — so
    /// the reference lands in the property's own array, not in a copy of it.
    fn ref_root_name(&mut self, b: &mut ChunkBuilder, root: &Expr) -> Result<String, String> {
        match root {
            Expr::Var(name) => Ok(name.clone()),
            Expr::PropGet(recv, prop) => {
                let tmp = self.tmp_name("ref");
                let (recv, prop) = (recv.as_ref().clone(), prop.clone());
                self.emit_set_var(b, &tmp, |c, b| {
                    c.compile_expr(b, &recv)?;
                    c.emit_member(b, &prop, 0)?;
                    b.emit(Op::CallBuiltin(ops::PROP_ENSURE_ARRAY, 2), c.cur_line);
                    Ok(())
                })?;
                Ok(tmp)
            }
            _ => Err("a reference path must be rooted at a variable or a property".into()),
        }
    }

    /// Compile one `unset()` target: a plain `$var` (remove the scope variable),
    /// an object property `$o->p` (remove the property, or call `__unset`), or an
    /// array element `$a[k1]..[kN]` (remove the deepest key).
    fn compile_unset_target(&mut self, b: &mut ChunkBuilder, t: &Expr) -> Result<(), String> {
        self.check_write_target(t)?;
        match t {
            Expr::Var(name) if name == "this" => {
                return Err(self.compile_fatal(self.cur_line, "Cannot unset $this"));
            }
            Expr::PropGet(recv, prop) => {
                self.compile_expr(b, recv)?;
                self.emit_member(b, prop, self.cur_line)?;
                b.emit(Op::CallBuiltin(ops::PROP_UNSET, 2), self.cur_line);
                b.emit(Op::Pop, self.cur_line);
            }
            Expr::Var(name) => {
                let idx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(idx), 0);
                b.emit(Op::CallBuiltin(ops::UNSET_VAR, 1), 0);
                b.emit(Op::Pop, 0);
            }
            // `unset($$x)` — same op, with the name computed rather than baked
            // into the chunk.
            Expr::VarVar(inner) => {
                self.compile_expr(b, inner)?;
                b.emit(Op::CallBuiltin(ops::UNSET_VAR, 1), 0);
                b.emit(Op::Pop, 0);
            }
            Expr::Index(..) => {
                let (root, segs) = Self::flatten_segments(t)?;
                // `unset($o->p[k])`: the property's array is reached through a
                // temporary that holds the handle itself — a plain `SETVAR`,
                // which does not copy — so removing the key removes it from the
                // property rather than from a copy of it.
                let name = match root {
                    Expr::Var(name) => name.clone(),
                    Expr::PropGet(..) | Expr::StaticProp(..) => {
                        let tmp = self.tmp_name("uns");
                        let root = root.clone();
                        self.emit_set_var(b, &tmp, |c, b| c.compile_expr(b, &root))?;
                        b.emit(Op::Pop, 0);
                        tmp
                    }
                    _ => {
                        return Err(
                            "unset() supports only `$var`, `$var[...]` and `$obj->prop[...]` \
                             targets"
                                .into(),
                        )
                    }
                };
                let nidx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(nidx), 0);
                for seg in &segs {
                    match seg {
                        LvSeg::Key(k) => self.compile_expr(b, k)?,
                        LvSeg::Append => return Err("cannot unset an `[]` append target".into()),
                    }
                }
                b.emit(
                    Op::CallBuiltin(ops::UNSET_PATH, (segs.len() + 1) as u8),
                    self.cur_line,
                );
                b.emit(Op::Pop, 0);
            }
            _ => return Err("unset() target must be a variable or an array element".into()),
        }
        Ok(())
    }

    /// `match (subj) { A, B => R, default => D }` — a value-producing expression.
    /// The subject is compared (`===`) against each arm's conditions; the first
    /// match's body value is left on the stack.
    fn compile_match(
        &mut self,
        b: &mut ChunkBuilder,
        subj: &Expr,
        arms: &[MatchArm],
    ) -> Result<(), String> {
        let m_t = self.tmp_name("m");
        self.emit_set_var(b, &m_t, |c, b| c.compile_expr(b, subj))?;

        // Dispatch: strict compare against every condition of every non-default
        // arm; the `default` arm is the fallback regardless of its position.
        let mut dispatch: Vec<(usize, usize)> = Vec::new(); // (arm index, JumpIfTrue pos)
        let mut default_index: Option<usize> = None;
        for (i, arm) in arms.iter().enumerate() {
            match &arm.conds {
                Some(conds) => {
                    for cond in conds {
                        self.emit_get_var(b, &m_t);
                        self.compile_expr(b, cond)?;
                        b.emit(Op::CallBuiltin(ops::STRICT_EQ, 2), 0);
                        b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0);
                        let jt = b.emit(Op::JumpIfTrue(0), 0);
                        dispatch.push((i, jt));
                    }
                }
                None => default_index = Some(i),
            }
        }
        // No arm matched: jump to the default body, or throw \UnhandledMatchError
        // with the unhandled subject in the message (PHP 8 semantics). The throw
        // halts this chunk, so there is no fall-through jump to the end.
        let default_jump = if default_index.is_some() {
            Some(b.emit(Op::Jump(0), 0))
        } else {
            let cls = b.add_constant(Value::str("UnhandledMatchError".to_string()));
            b.emit(Op::LoadConst(cls), 0);
            self.emit_get_var(b, &m_t);
            b.emit(Op::CallBuiltin(ops::MATCH_ERROR_MSG, 1), 0); // message string
            b.emit(Op::CallBuiltin(ops::NEW, 2), self.cur_line); // the exception object
            b.emit(Op::CallBuiltin(ops::THROW, 1), self.cur_line); // record + unwind
            None
        };

        // Arm bodies: each leaves exactly one value, then jumps to the end.
        let mut body_starts = Vec::with_capacity(arms.len());
        let mut body_ends = Vec::with_capacity(arms.len());
        for arm in arms {
            body_starts.push(b.current_pos());
            self.compile_expr(b, &arm.body)?;
            body_ends.push(b.emit(Op::Jump(0), 0));
        }
        let end = b.current_pos();

        for (i, jt) in dispatch {
            b.patch_jump(jt, body_starts[i]);
        }
        if let Some(di) = default_index {
            b.patch_jump(default_jump.unwrap(), body_starts[di]);
        }
        for j in body_ends {
            b.patch_jump(j, end);
        }
        Ok(())
    }

    /// An array literal with a `&` element, `[1, 'k' => &$a]`.
    ///
    /// Built in a temporary, element by element in source order, exactly as the
    /// equivalent sequence of writes would build it: `$t[k] = v` for a value and
    /// `$t[k] = &lv` for a reference, which binds the element and the lvalue to
    /// one slot. The literal's value is the temporary.
    fn compile_array_with_refs(
        &mut self,
        b: &mut ChunkBuilder,
        elems: &[ArrayElem],
    ) -> Result<(), String> {
        let t = self.tmp_name("refarr");
        let tv = || Box::new(Expr::Var(t.clone()));
        let init = Expr::Assign(
            tv(),
            None,
            Box::new(Expr::Array(Vec::new(), ArraySyntax::Short)),
        );
        self.compile_expr(b, &init)?;
        b.emit(Op::Pop, self.cur_line);
        for e in elems {
            if matches!(e.value, Expr::Spread(_)) {
                return Err("`...` and `&` in the same array literal are not supported".into());
            }
            let slot = match &e.key {
                Some(k) => Expr::Index(tv(), Box::new(k.clone())),
                None => Expr::Append(tv()),
            };
            let write = if e.by_ref {
                Expr::RefAssign(Box::new(slot), Box::new(e.value.clone()))
            } else {
                Expr::Assign(Box::new(slot), None, Box::new(e.value.clone()))
            };
            self.compile_expr(b, &write)?;
            b.emit(Op::Pop, self.cur_line);
        }
        self.emit_get_var(b, &t);
        Ok(())
    }

    /// A double-quoted string: concatenate its parts, always yielding a string.
    fn compile_interp(&mut self, b: &mut ChunkBuilder, parts: &[InterpPart]) -> Result<(), String> {
        let empty = b.add_constant(Value::str(String::new()));
        b.emit(Op::LoadConst(empty), 0);
        for part in parts {
            match part {
                InterpPart::Lit(s) => {
                    let idx = b.add_constant(Value::str(s.clone()));
                    b.emit(Op::LoadConst(idx), 0);
                }
                InterpPart::Expr(e) => self.compile_expr(b, e)?,
            }
            // Interpolation converts too, so it carries its line as well.
            b.emit(Op::CallBuiltin(ops::CONCAT, 2), self.cur_line);
        }
        Ok(())
    }

    fn compile_binary(
        &mut self,
        b: &mut ChunkBuilder,
        op: BinOp,
        l: &Expr,
        r: &Expr,
    ) -> Result<(), String> {
        // Short-circuit logical operators evaluate the right side conditionally.
        if matches!(op, BinOp::And | BinOp::Or) {
            self.compile_truthy(b, l)?;
            let short = if op == BinOp::And {
                b.emit(Op::JumpIfFalse(0), 0)
            } else {
                b.emit(Op::JumpIfTrue(0), 0)
            };
            self.compile_truthy(b, r)?;
            let jend = b.emit(Op::Jump(0), 0);
            let shortcut = b.current_pos();
            b.patch_jump(short, shortcut);
            b.emit(
                if op == BinOp::And {
                    Op::LoadFalse
                } else {
                    Op::LoadTrue
                },
                0,
            );
            let end = b.current_pos();
            b.patch_jump(jend, end);
            return Ok(());
        }

        // PCRE-style operand order is not the only thing a message can inherit
        // from a compiler. The reference SWAPS the operands of `*` when the left
        // is a compile-time constant and the right is not, so the constant lands
        // in the second slot — and that swap is observable twice over, because
        // the operands are also COERCED in slot order:
        //
        //     "g" * $t    →  Unsupported operand types: int * string   (swapped)
        //     "5g" * $t   →  throws with NO "non-numeric value" warning for "5g",
        //                    because $t is coerced first and throws before it
        //
        // `*` and the three bitwise operators `|`, `&`, `^`. `+` is not
        // swapped even though it commutes on numbers — it is also array union,
        // which does not. `-`, `/`, `%` and `**` all report in source order.
        let swap = matches!(
            op,
            BinOp::Mul | BinOp::BitOr | BinOp::BitAnd | BinOp::BitXor
        ) && is_const_operand(l)
            && is_definitely_runtime(r);
        if swap {
            self.compile_expr(b, r)?;
            self.compile_expr(b, l)?;
        } else {
            self.compile_expr(b, l)?;
            self.compile_expr(b, r)?;
        }
        match op {
            BinOp::Add => {
                b.emit(Op::Add, self.cur_line);
            }
            BinOp::Sub => {
                b.emit(Op::Sub, self.cur_line);
            }
            BinOp::Mul => {
                b.emit(Op::Mul, self.cur_line);
            }
            BinOp::Div => {
                b.emit(Op::CallBuiltin(ops::DIV, 2), self.cur_line);
            }
            BinOp::Mod => {
                b.emit(Op::CallBuiltin(ops::MOD, 2), self.cur_line);
            }
            BinOp::Pow => {
                b.emit(Op::CallBuiltin(ops::POW, 2), self.cur_line);
            }
            BinOp::Concat => {
                // The line matters: concatenation CONVERTS, and a conversion can
                // warn (`Array to string conversion`, and the NaN one).
                b.emit(Op::CallBuiltin(ops::CONCAT, 2), self.cur_line);
            }
            BinOp::LooseEq => {
                b.emit(Op::CallBuiltin(ops::LOOSE_EQ, 2), 0);
            }
            BinOp::LooseNe => {
                b.emit(Op::CallBuiltin(ops::LOOSE_NE, 2), 0);
            }
            BinOp::StrictEq => {
                b.emit(Op::CallBuiltin(ops::STRICT_EQ, 2), 0);
            }
            BinOp::StrictNe => {
                b.emit(Op::CallBuiltin(ops::STRICT_NE, 2), 0);
            }
            // The four relational operators are lowered NATIVELY, the way `+`
            // already is. fusevm answers an `Int`/`Int` or exact `Float` pair
            // itself and hands every other pair — a string, a bool, null, an
            // array — to the numeric hook, which applies PHP's own comparison.
            // The native answer and PHP's agree on exactly the pairs fusevm
            // keeps, so this changes no result; it removes the `CallBuiltin`
            // that made every loop condition untraceable.
            BinOp::Lt => {
                b.emit(Op::NumLt, self.cur_line);
            }
            BinOp::Gt => {
                b.emit(Op::NumGt, self.cur_line);
            }
            BinOp::Le => {
                b.emit(Op::NumLe, self.cur_line);
            }
            BinOp::Ge => {
                b.emit(Op::NumGe, self.cur_line);
            }
            BinOp::Spaceship => {
                b.emit(Op::CallBuiltin(ops::SPACESHIP, 2), 0);
            }
            BinOp::BitAnd => {
                b.emit(Op::CallBuiltin(ops::BITAND, 2), self.cur_line);
            }
            BinOp::BitOr => {
                b.emit(Op::CallBuiltin(ops::BITOR, 2), self.cur_line);
            }
            BinOp::BitXor => {
                b.emit(Op::CallBuiltin(ops::BITXOR, 2), self.cur_line);
            }
            BinOp::Shl => {
                b.emit(Op::CallBuiltin(ops::SHL, 2), self.cur_line);
            }
            BinOp::Shr => {
                b.emit(Op::CallBuiltin(ops::SHR, 2), self.cur_line);
            }
            BinOp::And | BinOp::Or => unreachable!("handled above"),
        }
        Ok(())
    }

    /// The right-hand side of an assignment: the value, then the copy PHP makes
    /// of it. An array is a value in PHP — `$b = $a` and `$o->p = $a` each store
    /// something the original cannot see writes to — and this is the one place
    /// that is true, which is why the copy rides on the *assignment* rather than
    /// on every write of a variable. See `ops::COPY`.
    fn compile_rhs(&mut self, b: &mut ChunkBuilder, rhs: &Expr) -> Result<(), String> {
        self.compile_expr(b, rhs)?;
        // The copy exists so `$b = $a` gives the two names separate ARRAYS. A
        // value that cannot be an array has nothing to copy, and the call is
        // both a wasted round trip through the host and — because it is a
        // `CallBuiltin` — the one op that stops a loop being traced.
        if !never_array(rhs) {
            b.emit(Op::CallBuiltin(ops::COPY, 1), 0);
        }
        Ok(())
    }

    /// The compile-time refusals of a written assignment, `++`/`--` or `unset`
    /// target: `zend_ensure_writable_variable`, then the base of its `[…]` /
    /// `->` chain compiled for WRITE ([`Self::check_write_base`]). Each is an
    /// `E_COMPILE_ERROR`, so nothing of the file runs.
    fn check_write_target(&self, target: &Expr) -> Result<(), String> {
        let fatal = |msg: &str| Err(self.compile_fatal(self.cur_line, msg));
        match target {
            Expr::Call(..) | Expr::CallValue(..) => {
                return fatal("Can't use function return value in write context");
            }
            Expr::MethodCall(..) | Expr::NullsafeMethodCall(..) | Expr::StaticCall(..) => {
                return fatal("Can't use method return value in write context");
            }
            _ => {}
        }
        if chain_has_nullsafe(target) {
            return fatal("Can't use nullsafe operator in write context");
        }
        self.check_write_base(target)
    }

    /// `zend_delayed_compile_dim` / `zend_delayed_compile_prop` under
    /// `BP_VAR_W`: the container of a written element or property is itself
    /// compiled for write, down the whole chain. A variable, a static property
    /// or a call can be written through; `zend_compile_var_inner` refuses
    /// anything else — `(new A)->x = 1`, `[1, 2][0] = 3`, `A::C[0] = 1` — as a
    /// temporary, and `clone`'s TMP result as a built-in function's.
    fn check_write_base(&self, e: &Expr) -> Result<(), String> {
        let (Expr::Index(base, _) | Expr::Append(base) | Expr::PropGet(base, _)) = e else {
            return Ok(());
        };
        match &**base {
            Expr::Index(..) | Expr::Append(..) | Expr::PropGet(..) => self.check_write_base(base),
            Expr::Var(_)
            | Expr::VarVar(_)
            | Expr::StaticProp(..)
            | Expr::Call(..)
            | Expr::CallValue(..)
            | Expr::MethodCall(..)
            | Expr::StaticCall(..) => Ok(()),
            Expr::Clone(_) => Err(self.compile_fatal(
                self.cur_line,
                "Cannot use result of built-in function in write context",
            )),
            _ => Err(self.compile_fatal(
                self.cur_line,
                "Cannot use temporary expression in write context",
            )),
        }
    }

    fn compile_assign(
        &mut self,
        b: &mut ChunkBuilder,
        lhs: &Expr,
        op: Option<BinOp>,
        rhs: &Expr,
    ) -> Result<(), String> {
        match lhs {
            // A compound `$this .= x` reads `$this` first and fails at run time;
            // a plain store is refused while compiling.
            Expr::Var(name) if name == "this" && op.is_none() => {
                return Err(self.compile_fatal(self.cur_line, "Cannot re-assign $this"));
            }
            Expr::Var(name) => {
                self.emit_var_target(b, name);
                match op {
                    None => self.compile_rhs(b, rhs)?,
                    Some(cop) => {
                        // $x <op>= rhs  ⇒  $x = $x <op> rhs. The value stored is
                        // the operator's result, which is always freshly made, so
                        // the assignment copy the plain `=` form needs would be
                        // protecting nothing here — and `$s += $i` in a counted
                        // loop pays for it every iteration.
                        self.emit_get_var(b, name);
                        self.compile_expr(b, rhs)?;
                        self.emit_binop(b, cop);
                    }
                }
                self.emit_store_var(b, name);
            }
            Expr::PropGet(recv, name) => {
                // `$o->p = rhs` and its compound form `$o->p op= rhs`. For a
                // compound op the receiver is evaluated ONCE into a temporary
                // (shared by the read and the write).
                match op {
                    None => {
                        self.compile_prop_write_recv(b, recv, false)?;
                        self.emit_member(b, name, 0)?;
                        self.compile_rhs(b, rhs)?;
                        b.emit(Op::CallBuiltin(ops::PROP_SET, 3), self.cur_line);
                    }
                    Some(cop) => {
                        let r = self.tmp_name("pr");
                        self.emit_set_var(b, &r, |c, b| c.compile_prop_write_recv(b, recv, true))?;
                        let name = &self.stash_member(b, name)?;
                        // Fetch-for-write comes FIRST, so `$o->missing .= "x"`
                        // deprecates the dynamic property before the read below
                        // warns that it is undefined — the reference order.
                        self.emit_get_var(b, &r);
                        self.emit_member(b, name, 0)?;
                        b.emit(Op::CallBuiltin(ops::PROP_TOUCH, 2), self.cur_line);
                        self.emit_member(b, name, 0)?;
                        // value = @r->name op rhs
                        self.emit_get_var(b, &r);
                        self.emit_member(b, name, 0)?;
                        b.emit(Op::CallBuiltin(ops::PROP_GET, 2), self.cur_line);
                        self.compile_rhs(b, rhs)?;
                        self.emit_binop(b, cop);
                        b.emit(Op::CallBuiltin(ops::PROP_SET_RW, 3), self.cur_line);
                    }
                }
            }
            Expr::Index(..) | Expr::Append(..) => {
                let (root, segs) = Self::flatten_segments(lhs)?;
                match root {
                    Expr::Var(name) => self.compile_lvalue_assign(b, name, &segs, op, rhs)?,
                    Expr::PropGet(recv, prop) => {
                        // Index/append into an array-valued property: vivify the
                        // property into an array, hold its handle in a temp, and
                        // write through it (arrays are reference handles, so the
                        // mutation lands on the object).
                        let t = self.tmp_name("po");
                        self.emit_set_var(b, &t, |c, b| {
                            c.compile_prop_write_recv(b, recv, op.is_some())?;
                            c.emit_member(b, prop, 0)?;
                            b.emit(Op::CallBuiltin(ops::PROP_ENSURE_ARRAY, 2), c.cur_line);
                            Ok(())
                        })?;
                        self.compile_lvalue_assign(b, &t, &segs, op, rhs)?;
                    }
                    // `Class::$p[k] = v`: the same, through the static's array.
                    Expr::StaticProp(class, prop) => {
                        let t = self.tmp_name("sp");
                        let (class, prop) = (class.clone(), prop.clone());
                        self.emit_set_var(b, &t, |c, b| {
                            c.emit_class_ref(b, &class)?;
                            let nidx = b.add_constant(Value::str(prop.clone()));
                            b.emit(Op::LoadConst(nidx), 0);
                            b.emit(Op::CallBuiltin(ops::SPROP_ENSURE_ARRAY, 2), c.cur_line);
                            Ok(())
                        })?;
                        self.compile_lvalue_assign(b, &t, &segs, op, rhs)?;
                    }
                    _ => return Err("unsupported assignment target".into()),
                }
            }
            Expr::StaticProp(class, name) => {
                // `Class::$p = rhs` and its compound form `Class::$p op= rhs`.
                self.emit_class_ref(b, class)?;
                let nidx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(nidx), 0);
                match op {
                    None => self.compile_rhs(b, rhs)?,
                    Some(cop) => {
                        // value = Class::$p op rhs
                        self.compile_expr(b, lhs)?;
                        self.compile_rhs(b, rhs)?;
                        self.emit_binop(b, cop);
                    }
                }
                b.emit(Op::CallBuiltin(ops::SPROP_SET, 3), self.cur_line);
            }
            // List destructuring — `list($a,$b) = …`, `[$a,$b] = …`, and the keyed
            // form `['k' => $v] = …`. Both `list(...)` and `[...]` parse to
            // `Expr::Array`, so this one arm serves every syntax. The RHS is
            // evaluated once into a temp (its value is also the assignment
            // expression's result, matching PHP), then each element target is
            // assigned `@src[key]`. Unkeyed elements take successive integer
            // indices; a `Null` element is a hole (`[,$b]`) that still consumes an
            // index but binds nothing; a nested `Expr::Array` target recurses.
            Expr::Array(elems, syntax) => {
                if op.is_some() {
                    return Err("compound assignment cannot target a list()/[] pattern".into());
                }
                self.check_list_pattern(elems, *syntax)?;
                // A `&` target aliases the SUBJECT, so it needs a subject a
                // reference can point into. Against a literal PHP refuses at
                // COMPILE time — `echo "pre"; [&$x] = [1, 2];` prints nothing
                // before the fatal, because the whole file is compiled before
                // any of it runs — so this is rejected here rather than emitted
                // as an op. (The reverse of `ops::DECL_FATAL`, which is an op
                // precisely because PHP's trait fatal lands at run time.)
                let ref_root = Self::ref_source(rhs);
                if ref_root.is_none() && Self::pattern_binds_by_ref(elems) && is_literal(rhs) {
                    return Err("Cannot assign reference to non referenceable value".into());
                }
                let src = self.tmp_name("list");
                self.emit_set_var(b, &src, |c, b| c.compile_rhs(b, rhs))?;
                self.compile_list_targets(b, elems, &src, ref_root)?;
                // The whole `[...] = rhs` expression evaluates to the RHS value.
                self.emit_get_var(b, &src);
            }
            // `$$x = v` / `${expr} = v`: push the computed NAME, then the value.
            Expr::VarVar(inner) => {
                self.compile_expr(b, inner)?;
                match op {
                    None => self.compile_rhs(b, rhs)?,
                    Some(cop) => {
                        // The name expression is evaluated once and reused for
                        // the read and the write, so a side-effecting operand
                        // runs once, as it does for every other compound target.
                        b.emit(Op::Dup, 0);
                        b.emit(Op::CallBuiltin(ops::GETVAR, 1), self.cur_line);
                        self.compile_expr(b, rhs)?;
                        self.emit_binop(b, cop);
                    }
                }
                b.emit(Op::CallBuiltin(ops::SETVAR, 2), self.cur_line);
            }
            _ => return Err("invalid assignment target".into()),
        }
        Ok(())
    }

    /// The subject a by-reference destructuring target aliases INTO.
    ///
    /// `[&$x] = $a` binds `$x` to `$a[0]`, so the reference has to be taken
    /// against the ORIGINAL subject and never against the temp the pattern
    /// copies it into — writing through the temp would be invisible in `$a`,
    /// which is the whole observable effect of the `&`. Only an lvalue can
    /// serve: a variable, an array element, or an object property.
    fn ref_source(rhs: &Expr) -> Option<&Expr> {
        match rhs {
            Expr::Var(_) | Expr::Index(..) | Expr::PropGet(..) => Some(rhs),
            _ => None,
        }
    }

    /// Whether any target in this pattern is by reference, at any depth — a
    /// nested `[[&$x]]` needs the subject to be referenceable just as much as a
    /// flat one does.
    fn pattern_binds_by_ref(elems: &[ArrayElem]) -> bool {
        elems.iter().any(|e| {
            e.by_ref
                || match &e.value {
                    Expr::Array(inner, _) => Self::pattern_binds_by_ref(inner),
                    _ => false,
                }
        })
    }

    /// The compile-time checks `zend_compile_list_assign` makes on a
    /// destructuring pattern, in its order: per element, a gap in a keyed
    /// pattern, a spread, keyed/unkeyed mixing, then the target itself (a nested
    /// pattern must share its parent's spelling and may not be `array()`, and is
    /// checked in full before the next element); finally a pattern binding
    /// nothing is an empty list. Each is an `E_COMPILE_ERROR`, so it fires
    /// before any of the file runs.
    fn check_list_pattern(&self, elems: &[ArrayElem], style: ArraySyntax) -> Result<(), String> {
        let fatal = |msg: &str| Err(self.compile_fatal(self.cur_line, msg));
        let is_keyed = elems
            .first()
            .is_some_and(|e| !matches!(e.value, Expr::Hole) && e.key.is_some());
        let mut has_elems = false;
        for e in elems {
            match &e.value {
                Expr::Hole if is_keyed => {
                    return fatal("Cannot use empty array entries in keyed array assignment");
                }
                Expr::Hole => continue,
                Expr::Spread(_) => return fatal("Spread operator is not supported in assignments"),
                _ => {}
            }
            has_elems = true;
            if e.key.is_some() != is_keyed {
                return fatal("Cannot mix keyed and unkeyed array entries in assignments");
            }
            match &e.value {
                Expr::Array(_, ArraySyntax::Long) => {
                    return fatal("Cannot assign to array(), use [] instead");
                }
                Expr::Array(_, syntax) if *syntax != style => {
                    return fatal("Cannot mix [] and list()");
                }
                Expr::Array(inner, syntax) => self.check_list_pattern(inner, *syntax)?,
                Expr::Var(n) if n == "this" => return fatal("Cannot re-assign $this"),
                // `zend_ensure_writable_variable`: a call is a variable to the
                // grammar but not a place to store into.
                Expr::Call(..) | Expr::CallValue(..) => {
                    return fatal("Can't use function return value in write context");
                }
                Expr::MethodCall(..) | Expr::StaticCall(..) => {
                    return fatal("Can't use method return value in write context");
                }
                target if !list_target_writable(target) => {
                    return fatal("Assignments can only happen to writable values");
                }
                _ => {}
            }
        }
        if !has_elems {
            return fatal("Cannot use empty list");
        }
        Ok(())
    }

    /// Assign each target of a destructuring pattern.
    ///
    /// Two sources are threaded, not one. `value_tmp` names the temp holding a
    /// COPY of the subject, which every by-value target reads — that is what
    /// makes `[$x] = $a; $x = 9;` leave `$a` alone. `ref_path` is the path to
    /// the ORIGINAL subject, present only when it is referenceable, and it is
    /// what a `&` target aliases. Recursion deepens both in step so a nested
    /// `[[&$x]] = $a` still reaches `$a[0][0]` rather than a copy of it.
    fn compile_list_targets(
        &mut self,
        b: &mut ChunkBuilder,
        elems: &[ArrayElem],
        value_tmp: &str,
        ref_path: Option<&Expr>,
    ) -> Result<(), String> {
        let mut counter: i64 = 0;
        for e in elems {
            let key = match &e.key {
                Some(ke) => ke.clone(),
                None => {
                    let i = counter;
                    counter += 1;
                    Expr::Int(i)
                }
            };
            // A hole binds nothing but has already consumed its index.
            if matches!(e.value, Expr::Hole) {
                continue;
            }
            let elem = Expr::ListElem(
                Box::new(Expr::Var(value_tmp.to_string())),
                Box::new(key.clone()),
            );
            let deeper = ref_path.map(|p| Expr::Index(Box::new(p.clone()), Box::new(key.clone())));
            match &e.value {
                // A nested pattern recurses, carrying both sources down.
                Expr::Array(inner, _) => {
                    let inner_tmp = self.tmp_name("list");
                    self.emit_set_var(b, &inner_tmp, |c, b| c.compile_expr(b, &elem))?;
                    self.compile_list_targets(b, inner, &inner_tmp, deeper.as_ref())?;
                }
                // `&$x` — alias the subject's element rather than copy it.
                // Without a referenceable subject there is nothing to alias, so
                // the target falls back to the copy (PHP notices and does the
                // same for a subject it cannot reference, such as a call
                // result).
                target if e.by_ref => match &deeper {
                    Some(source) => {
                        self.compile_ref_assign(b, target, source)?;
                        b.emit(Op::Pop, 0);
                    }
                    None => {
                        self.compile_assign(b, target, None, &elem)?;
                        b.emit(Op::Pop, 0);
                    }
                },
                target => {
                    self.compile_assign(b, target, None, &elem)?;
                    b.emit(Op::Pop, 0);
                }
            }
        }
        Ok(())
    }

    /// Flatten a nested array lvalue (`$a[k1][k2]...`, with any `[]` appends) into
    /// its root expression (a `$var` or an `$o->prop`) and the ordered chain of
    /// segments (source order).
    fn flatten_segments(lhs: &Expr) -> Result<(&Expr, Vec<LvSeg<'_>>), String> {
        let mut cur = lhs;
        let mut segs: Vec<LvSeg> = Vec::new();
        loop {
            match cur {
                Expr::Index(recv, idx) => {
                    segs.push(LvSeg::Key(idx));
                    cur = recv;
                }
                Expr::Append(inner) => {
                    segs.push(LvSeg::Append);
                    cur = inner;
                }
                other => {
                    segs.reverse();
                    return Ok((other, segs));
                }
            }
        }
    }

    /// Assign along a flattened lvalue chain. A `[]` that is not the outermost
    /// segment (`$a[][k] = v`) is materialized by appending a fresh child array and
    /// re-rooting the remaining segments on it through a temporary, so each `[]`
    /// appends exactly one element as PHP does. Otherwise the flat name+keys(+
    /// trailing append) fast paths are used.
    fn compile_lvalue_assign(
        &mut self,
        b: &mut ChunkBuilder,
        name: &str,
        segs: &[LvSeg],
        op: Option<BinOp>,
        rhs: &Expr,
    ) -> Result<(), String> {
        // The first append that still has segments after it is a mid-path append.
        let mid = segs
            .iter()
            .position(|s| matches!(s, LvSeg::Append))
            .filter(|&i| i + 1 < segs.len());
        if let Some(i) = mid {
            // Everything before the first append is a plain key prefix.
            let prefix: Vec<&Expr> = segs[..i]
                .iter()
                .map(|s| match s {
                    LvSeg::Key(k) => *k,
                    LvSeg::Append => unreachable!("first mid-append has no earlier append"),
                })
                .collect();
            let t = self.tmp_name("ap");
            // @t = a freshly appended child array of $name[prefix...].
            self.emit_set_var(b, &t, |c, b| {
                let nidx = b.add_constant(Value::str(name.to_string()));
                b.emit(Op::LoadConst(nidx), 0);
                for k in &prefix {
                    c.compile_expr(b, k)?;
                }
                b.emit(
                    Op::CallBuiltin(ops::PATH_APPEND_CHILD, (prefix.len() + 1) as u8),
                    0,
                );
                Ok(())
            })?;
            // Keep writing through $@t along the remaining segments.
            return self.compile_lvalue_assign(b, &t, &segs[i + 1..], op, rhs);
        }

        // No mid-append: keys with an optional trailing `[]` append.
        let append = matches!(segs.last(), Some(LvSeg::Append));
        let key_segs = if append {
            &segs[..segs.len() - 1]
        } else {
            segs
        };
        let keys: Vec<&Expr> = key_segs
            .iter()
            .map(|s| match s {
                LvSeg::Key(k) => *k,
                LvSeg::Append => unreachable!("only the final segment may be an append here"),
            })
            .collect();

        if append {
            if op.is_some() {
                return Err("`[]` append takes only plain `=`".into());
            }
            let nidx = b.add_constant(Value::str(name.to_string()));
            b.emit(Op::LoadConst(nidx), 0);
            if keys.is_empty() {
                // Single-level `$a[] = rhs` keeps the compact ARR_APPEND lowering.
                self.compile_rhs(b, rhs)?;
                b.emit(Op::CallBuiltin(ops::ARR_APPEND, 2), 0);
            } else {
                for k in &keys {
                    self.compile_expr(b, k)?;
                }
                self.compile_rhs(b, rhs)?;
                b.emit(Op::CallBuiltin(ops::APPEND_PATH, (keys.len() + 2) as u8), 0);
            }
        } else if keys.len() == 1 && op.is_none() {
            // Fast path for the common single-level `$a[k] = rhs`.
            let nidx = b.add_constant(Value::str(name.to_string()));
            b.emit(Op::LoadConst(nidx), 0);
            self.compile_expr(b, keys[0])?;
            self.compile_rhs(b, rhs)?;
            b.emit(Op::CallBuiltin(ops::INDEX_SET, 3), self.cur_line);
        } else {
            self.compile_index_assign(b, name, &keys, op, rhs)?;
        }
        Ok(())
    }

    /// `$a[k1]..[kN] = rhs` (deep set) and its compound form `$a[k1]..[kN] op=
    /// rhs`. For a compound op the key expressions are hoisted into temporaries so
    /// they are evaluated exactly once across the read and the write.
    fn compile_index_assign(
        &mut self,
        b: &mut ChunkBuilder,
        name: &str,
        keys: &[&Expr],
        op: Option<BinOp>,
        rhs: &Expr,
    ) -> Result<(), String> {
        let nidx = b.add_constant(Value::str(name.to_string()));
        match op {
            None => {
                b.emit(Op::LoadConst(nidx), 0);
                for k in keys {
                    self.compile_expr(b, k)?;
                }
                self.compile_rhs(b, rhs)?;
                b.emit(Op::CallBuiltin(ops::SET_PATH, (keys.len() + 2) as u8), 0);
            }
            Some(cop) => {
                // Evaluate each key once into a temporary, then `set = get op rhs`.
                let key_tmps: Vec<String> = keys.iter().map(|_| self.tmp_name("lk")).collect();
                for (t, k) in key_tmps.iter().zip(keys) {
                    self.emit_set_var(b, t, |c, b| c.compile_expr(b, k))?;
                }
                b.emit(Op::LoadConst(nidx), 0);
                for t in &key_tmps {
                    self.emit_get_var(b, t);
                }
                // value = $a[keys] op rhs
                b.emit(Op::LoadConst(nidx), 0);
                for t in &key_tmps {
                    self.emit_get_var(b, t);
                }
                b.emit(
                    Op::CallBuiltin(ops::GET_PATH, (key_tmps.len() + 1) as u8),
                    self.cur_line,
                );
                self.compile_rhs(b, rhs)?;
                self.emit_binop(b, cop);
                b.emit(
                    Op::CallBuiltin(ops::SET_PATH, (key_tmps.len() + 2) as u8),
                    0,
                );
            }
        }
        Ok(())
    }

    /// Lower PHP 8.1 first-class callable syntax, `callee(...)`, to a real
    /// `Closure` over the callable — with the callable settled HERE.
    ///
    /// Two things have to happen at the syntax, not at the eventual call, and
    /// both are why this is not simply the desugared arrow function the parser
    /// used to build:
    ///
    /// * the callable expression is evaluated ONCE, so `f()->m(...)` runs `f`
    ///   exactly once however often the closure is later invoked (the arrow
    ///   function rebuilt `[f(), "m"]` on every call);
    /// * the callee is CHECKED, so `$o->nope(...)` raises where it is written
    ///   even though nothing calls the closure.
    ///
    /// The value is parked in a temporary, which the arrow function then
    /// captures by name like any other free variable — so the closure body,
    /// `fn(...$args) => call_user_func_array($tmp, $args)`, holds the settled
    /// value rather than the expression that produced it.
    fn compile_fcc(
        &mut self,
        b: &mut ChunkBuilder,
        callable: &Expr,
        instance: bool,
    ) -> Result<(), String> {
        let tmp = self.tmp_name("fcc");
        self.emit_set_var(b, &tmp, |c, b| {
            c.compile_expr(b, callable)?;
            b.emit(
                if instance {
                    Op::LoadTrue
                } else {
                    Op::LoadFalse
                },
                0,
            );
            b.emit(Op::CallBuiltin(ops::FCC_CHECK, 2), c.cur_line);
            Ok(())
        })?;
        let param = Param {
            name: "args".to_string(),
            line: self.cur_line,
            ty: None,
            default: None,
            variadic: true,
            promoted: false,
            promoted_vis: Visibility::Public,
            readonly: false,
            by_ref: false,
        };
        // `call_user_func_array` reads an integer key as a positional argument
        // and a string key as a named one, which is what carries `$f(b: 2)`
        // through to the parameter it names.
        let body = Expr::Call(
            "call_user_func_array".to_string(),
            vec![Expr::Var(tmp), Expr::Var("args".to_string())],
        );
        self.compile_expr(
            b,
            &Expr::ArrowFn {
                params: vec![param],
                body: Box::new(body),
                ret: None,
                is_static: false,
                // The closure the reference synthesizes for `f(...)` is written
                // where the syntax is, so that is the line its frames name.
                line: self.cur_line,
            },
        )
    }

    /// Lower an anonymous function / arrow function to a closure-creating
    /// sequence: compile the body into its own chunk (registered under a synthetic
    /// name in the function table, with its parameters so defaults/variadics bind),
    /// then emit `MKCLOSURE` with the captured `(name, value)` pairs read from the
    /// current scope.
    ///
    /// Eight parameters because a closure literal carries eight independent
    /// facts from the parser — params, captures, body, return hint, `static`,
    /// and the line that names its frames — and bundling them into a struct
    /// would only move the same list one level out.
    #[allow(clippy::too_many_arguments)]
    fn compile_closure(
        &mut self,
        b: &mut ChunkBuilder,
        params: &[Param],
        captures: &[Capture],
        body: &[Stmt],
        ret: Option<&TypeHint>,
        is_static: bool,
        line: u32,
    ) -> Result<(), String> {
        let script = host::with_host(|h| h.script_name().to_string());
        let owner = format!("{{closure:{}:{line}}}()", self.decl_site.render(&script));
        let cparams = self.compile_params(params, &owner)?;
        let mut fb = ChunkBuilder::new();
        // Like a named function, the body gets its own loop scope so a `break`
        // inside it cannot target a loop at the creation site.
        let saved = std::mem::take(&mut self.loops);
        let saved_try = self.enter_own_loop_scope();
        // This literal's own site, which names its frames — and which a closure
        // written INSIDE it nests under, so `{closure:{closure:f.php:2}:3}`
        // falls out of the same rule rather than being a second case.
        let site = host::DeclSite::Closure(Box::new(self.decl_site.clone()), line);
        let saved_site = std::mem::replace(&mut self.decl_site, site.clone());
        self.in_other_frame(|c| c.compile_seq(&mut fb, body))?;
        self.resolve_gotos(&mut fb, self.cur_chunk, true)?;
        self.decl_site = saved_site;
        self.loops = saved;
        self.leave_own_loop_scope(saved_try);
        // The prelude is merged onto every host BEFORE the user program, whose
        // own closures count from the same `@closure1`; a name of its own keeps a
        // user closure from replacing the prelude's in the function table.
        let def_name = self.tmp_name(if self.prelude {
            "prelude_closure"
        } else {
            "closure"
        });
        self.functions.push((
            def_name.clone(),
            FuncDef {
                params: cparams,
                chunk: fb.build(),
                is_generator: body_has_yield(body),
                ret: ret.cloned(),
                // A closure frame is seeded from its captures, not from a
                // compiled local list, so its body stays by-name.
                locals: Vec::new(),
                closure_site: Some(site),
                declared: None,
                deprecated: None,
            },
        ));

        let nidx = b.add_constant(Value::str(def_name));
        b.emit(Op::LoadConst(nidx), 0);
        if is_static {
            // A `static` closure travels as an ordinary capture under a name no
            // PHP variable can have, which is how the creation site tells the
            // host to withhold `$this`. The class SCOPE still passes, so a
            // private static stays reachable — only the instance is withheld.
            let kidx = b.add_constant(Value::str(host::STATIC_CLOSURE_CAPTURE.to_string()));
            b.emit(Op::LoadConst(kidx), 0);
            b.emit(Op::LoadTrue, 0);
        }
        for cap in captures {
            let cidx = b.add_constant(Value::str(cap.name.clone()));
            b.emit(Op::LoadConst(cidx), 0);
            if cap.by_ref {
                // `use (&$v)` captures a handle to the enclosing variable's
                // reference cell, so the closure and the enclosing scope are
                // two names for one value however either one writes it.
                let nidx = b.add_constant(Value::str(cap.name.clone()));
                b.emit(Op::LoadConst(nidx), 0);
                b.emit(Op::CallBuiltin(ops::REF_CELL, 1), 0);
            } else if cap.name == "this" {
                // `$this` is not a `use` capture in PHP: it is bound implicitly,
                // at the closure's *call*, and an arrow function written outside a
                // method is legal until `Closure::bind` supplies one. phplang
                // carries it through the capture list, so this read has to be the
                // quiet one — the loud twin would report a variable PHP never
                // considers the closure to have read.
                self.compile_quiet(b, &Expr::Var("this".to_string()))?;
            } else {
                self.emit_get_var(b, &cap.name);
            }
        }
        b.emit(
            Op::CallBuiltin(
                ops::MKCLOSURE,
                (1 + (captures.len() + usize::from(is_static)) * 2) as u8,
            ),
            0,
        );
        Ok(())
    }

    fn compile_incdec(
        &mut self,
        b: &mut ChunkBuilder,
        target: &Expr,
        inc: bool,
        prefix: bool,
    ) -> Result<(), String> {
        // code: bit0 = increment, bit1 = prefix.
        let code = (inc as i64) | ((prefix as i64) << 1);
        match target {
            // A promoted local does its own read and write, and asks the host
            // only for the step. `$x++` yields the OLD value and `++$x` the new,
            // which is the only thing the two orderings below differ in.
            // A promoted local that only ever holds a number: `++` is exactly
            // `+ 1` for it, and `+` is a native op. This is what lets the
            // ordinary `for ($i = 0; $i < n; $i++)` be traced — the host step
            // below is a `CallBuiltin`, and one of those anywhere in a loop
            // body is enough for fusevm to decline the whole loop.
            Expr::Var(name) if self.fslots.contains_key(name) && self.fnumeric.contains(name) => {
                let i = self.fslots[name];
                b.emit(Op::GetSlot(i), self.cur_line);
                if prefix {
                    b.emit(Op::LoadInt(1), 0);
                    b.emit(if inc { Op::Add } else { Op::Sub }, self.cur_line);
                    b.emit(Op::Dup, 0);
                    b.emit(Op::SetSlot(i), self.cur_line);
                } else {
                    b.emit(Op::Dup, 0);
                    b.emit(Op::LoadInt(1), 0);
                    b.emit(if inc { Op::Add } else { Op::Sub }, self.cur_line);
                    b.emit(Op::SetSlot(i), self.cur_line);
                }
            }
            Expr::Var(name) if self.fslots.contains_key(name) => {
                let i = self.fslots[name];
                b.emit(Op::GetSlot(i), self.cur_line);
                if prefix {
                    b.emit(Op::LoadInt(i64::from(inc)), 0);
                    b.emit(Op::CallBuiltin(ops::INCDEC_STEP, 2), self.cur_line);
                    b.emit(Op::Dup, 0);
                    b.emit(Op::SetSlot(i), self.cur_line);
                } else {
                    b.emit(Op::Dup, 0);
                    b.emit(Op::LoadInt(i64::from(inc)), 0);
                    b.emit(Op::CallBuiltin(ops::INCDEC_STEP, 2), self.cur_line);
                    b.emit(Op::SetSlot(i), self.cur_line);
                }
            }
            Expr::Var(name) => match self.slots.get(name) {
                Some(&i) => {
                    b.emit(Op::LoadInt(i as i64), 0);
                    b.emit(Op::LoadInt(code), 0);
                    b.emit(Op::CallBuiltin(ops::INCDEC_SLOT, 2), self.cur_line);
                }
                None => {
                    let nidx = b.add_constant(Value::str(name.clone()));
                    b.emit(Op::LoadConst(nidx), 0);
                    b.emit(Op::LoadInt(code), 0);
                    b.emit(Op::CallBuiltin(ops::INCDEC, 2), self.cur_line);
                }
            },
            Expr::PropGet(recv, name) => {
                // `$o->p++` — read-modify-write a scalar property.
                self.compile_prop_write_recv(b, recv, true)?;
                self.emit_member(b, name, 0)?;
                b.emit(Op::LoadInt(code), 0);
                b.emit(Op::CallBuiltin(ops::PROP_INCDEC, 3), self.cur_line);
            }
            Expr::StaticProp(class, name) => {
                // `Class::$p++` — read-modify-write a static property.
                self.emit_class_ref(b, class)?;
                let nidx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(nidx), 0);
                b.emit(Op::LoadInt(code), 0);
                b.emit(Op::CallBuiltin(ops::SPROP_INCDEC, 3), self.cur_line);
            }
            Expr::Index(..) => {
                // `++$a[k1]..[kN]` — read-modify-write the deepest element. Roots
                // at a `$var` or an array-valued `$o->prop` (vivified into a temp).
                let (root, segs) = Self::flatten_segments(target)?;
                let mut keys: Vec<&Expr> = Vec::with_capacity(segs.len());
                for s in &segs {
                    match s {
                        LvSeg::Key(k) => keys.push(k),
                        LvSeg::Append => return Err("cannot ++/-- an `[]` append target".into()),
                    }
                }
                let name: String = match root {
                    Expr::Var(name) => name.clone(),
                    Expr::PropGet(recv, prop) => {
                        let t = self.tmp_name("po");
                        self.emit_set_var(b, &t, |c, b| {
                            c.compile_expr(b, recv)?;
                            c.emit_member(b, prop, 0)?;
                            b.emit(Op::CallBuiltin(ops::PROP_ENSURE_ARRAY, 2), c.cur_line);
                            Ok(())
                        })?;
                        t
                    }
                    Expr::StaticProp(class, prop) => {
                        let t = self.tmp_name("sp");
                        let (class, prop) = (class.clone(), prop.clone());
                        self.emit_set_var(b, &t, |c, b| {
                            c.emit_class_ref(b, &class)?;
                            let nidx = b.add_constant(Value::str(prop.clone()));
                            b.emit(Op::LoadConst(nidx), 0);
                            b.emit(Op::CallBuiltin(ops::SPROP_ENSURE_ARRAY, 2), c.cur_line);
                            Ok(())
                        })?;
                        t
                    }
                    _ => return Err("unsupported ++/-- target".into()),
                };
                let nidx = b.add_constant(Value::str(name));
                b.emit(Op::LoadConst(nidx), 0);
                for k in &keys {
                    self.compile_expr(b, k)?;
                }
                b.emit(Op::LoadInt(code), 0);
                b.emit(
                    Op::CallBuiltin(ops::INCDEC_PATH, (keys.len() + 2) as u8),
                    self.cur_line,
                );
            }
            _ => {
                return Err(
                    "scaffold supports ++/-- only on variables, array elements, and properties"
                        .into(),
                )
            }
        }
        Ok(())
    }

    /// Emit the native op for an arithmetic operator, or a builtin call for the
    /// PHP-semantic ones. Used by compound assignment.
    fn emit_binop(&mut self, b: &mut ChunkBuilder, op: BinOp) {
        match op {
            BinOp::Add => {
                b.emit(Op::Add, self.cur_line);
            }
            BinOp::Sub => {
                b.emit(Op::Sub, self.cur_line);
            }
            BinOp::Mul => {
                b.emit(Op::Mul, self.cur_line);
            }
            BinOp::Div => {
                b.emit(Op::CallBuiltin(ops::DIV, 2), self.cur_line);
            }
            BinOp::Mod => {
                b.emit(Op::CallBuiltin(ops::MOD, 2), self.cur_line);
            }
            BinOp::Pow => {
                b.emit(Op::CallBuiltin(ops::POW, 2), self.cur_line);
            }
            BinOp::Concat => {
                // The line matters: concatenation CONVERTS, and a conversion can
                // warn (`Array to string conversion`, and the NaN one).
                b.emit(Op::CallBuiltin(ops::CONCAT, 2), self.cur_line);
            }
            BinOp::BitAnd => {
                b.emit(Op::CallBuiltin(ops::BITAND, 2), self.cur_line);
            }
            BinOp::BitOr => {
                b.emit(Op::CallBuiltin(ops::BITOR, 2), self.cur_line);
            }
            BinOp::BitXor => {
                b.emit(Op::CallBuiltin(ops::BITXOR, 2), self.cur_line);
            }
            BinOp::Shl => {
                b.emit(Op::CallBuiltin(ops::SHL, 2), self.cur_line);
            }
            BinOp::Shr => {
                b.emit(Op::CallBuiltin(ops::SHR, 2), self.cur_line);
            }
            _ => unreachable!("compound assignment only uses arithmetic/bitwise/concat ops"),
        }
    }

    fn compile_truthy(&mut self, b: &mut ChunkBuilder, e: &Expr) -> Result<(), String> {
        self.compile_expr(b, e)?;
        // A comparison already answers with a bool, so coercing it is a host
        // round-trip that cannot change the value — and a loop condition pays
        // for it on every iteration.
        if !yields_bool(e) {
            b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0);
        }
        Ok(())
    }

    /// Lower an access chain whose spine spells a `?->`.
    ///
    /// The short-circuit extent is the WHOLE remaining chain, not the one link
    /// that spelled the operator: `$n?->a->b->c()` on a null `$n` is `NULL`
    /// with no diagnostic and no call, because the reference stops evaluating
    /// at the `?->` and resumes only after the last link. Lowering each link
    /// with its own two-branch merge instead — which is what this did — read
    /// `->b` off the null the first link produced, so every following link
    /// raised `Attempt to read property` and a method link was an uncaught
    /// `Call to a member function on null`.
    ///
    /// One value is left on the stack either way: the short-circuit jump goes
    /// out with the null receiver already there, and each link consumes one
    /// value and pushes one.
    ///
    /// `quiet` is PHP's `BP_VAR_IS` fetch mode (the operand of `isset`/`??`/`@`),
    /// which applies to every link of the chain exactly as [`Compiler::compile_quiet`]
    /// applies it to a chain without a `?->`.
    fn compile_nullsafe_chain(
        &mut self,
        b: &mut ChunkBuilder,
        e: &Expr,
        quiet: bool,
    ) -> Result<(), String> {
        // Walk the spine outward-in, then emit base-first.
        let mut spine = Vec::new();
        let mut base = e;
        while let Some(r) = chain_recv(base) {
            spine.push(base);
            base = r;
        }
        // `BP_VAR_IS` reaches inward only through property and index fetches: a
        // method call's receiver is fetched for READING, so in
        // `$r?->a->m() ?? d` the `->a` warns. Links inside the outermost call
        // (larger spine index — the spine runs outermost first) are loud.
        let call_at = spine
            .iter()
            .position(|l| matches!(l, Expr::MethodCall(..) | Expr::NullsafeMethodCall(..)));
        let quiet_at = |i: usize| quiet && call_at.map_or(true, |c| i < c);
        if quiet_at(spine.len()) {
            self.compile_quiet(b, base)?;
        } else {
            self.compile_expr(b, base)?;
        }
        // Pending jumps to the chain's end — one per `?->` that short-circuits.
        let mut exits = Vec::new();
        for (i, link) in spine.iter().enumerate().rev() {
            if matches!(
                link,
                Expr::NullsafePropGet(..) | Expr::NullsafeMethodCall(..)
            ) {
                b.emit(Op::Dup, 0); // [recv, recv]
                b.emit(Op::LoadUndef, 0); // [recv, recv, null]
                b.emit(Op::CallBuiltin(ops::STRICT_EQ, 2), 0); // [recv, isNull]
                b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0); // [recv, bool]
                exits.push(b.emit(Op::JumpIfTrue(0), 0));
            }
            self.compile_chain_link(b, link, quiet_at(i))?;
        }
        let end = b.current_pos();
        for j in exits {
            b.patch_jump(j, end);
        }
        Ok(())
    }

    /// Lower ONE link of an access chain, with its receiver already on the
    /// stack. The nullsafe spelling of a link accesses exactly as the plain one
    /// does — the operator's only effect is the short-circuit its caller emits.
    fn compile_chain_link(
        &mut self,
        b: &mut ChunkBuilder,
        link: &Expr,
        quiet: bool,
    ) -> Result<(), String> {
        let line = self.cur_line;
        match link {
            Expr::PropGet(_, name) | Expr::NullsafePropGet(_, name) => {
                self.emit_member(b, name, line)?;
                let op = if quiet {
                    ops::PROP_GET_Q
                } else {
                    ops::PROP_GET
                };
                b.emit(Op::CallBuiltin(op, 2), line);
            }
            Expr::Index(_, idx) => {
                self.compile_expr(b, idx)?;
                let op = if quiet {
                    ops::INDEX_GET_Q
                } else {
                    ops::INDEX_GET
                };
                b.emit(Op::CallBuiltin(op, 2), line);
            }
            // A method CALL is never quietened: `isset()` asks about a storage
            // location, and the call that produced the value already ran.
            Expr::MethodCall(_, name, args) | Expr::NullsafeMethodCall(_, name, args) => {
                let name = &self.method_member(b, name, args.len())?;
                // The receiver is judged before the arguments here too — a `?->`
                // short-circuits only on NULL, so a `false`/int/array receiver
                // still reaches the call and must reject it before an argument
                // has the chance to print anything.
                self.emit_mcall_recv_check(b, name, args.len())?;
                self.emit_member(b, name, line)?;
                if needs_arg_pairs(args) {
                    self.compile_arg_pairs(b, args)?;
                    b.emit(
                        Op::CallBuiltin(ops::MCALL_NAMED, (args.len() * 2 + 2) as u8),
                        line,
                    );
                } else {
                    for a in args {
                        self.compile_expr(b, a)?;
                    }
                    b.emit(Op::CallBuiltin(ops::MCALL, (args.len() + 2) as u8), line);
                    let all = (0..args.len()).collect::<Vec<_>>();
                    self.emit_byref_writeback(b, args, &all, true)?;
                }
            }
            other => return Err(format!("not an access-chain link: {other:?}")),
        }
        Ok(())
    }

    /// Push each call argument as a `(name, value)` pair for a `*_NAMED` call: a
    /// named argument contributes its name as a string constant, a positional
    /// argument contributes `Undef`. Consumed by the host's named-argument binding.
    fn compile_arg_pairs(&mut self, b: &mut ChunkBuilder, args: &[Expr]) -> Result<(), String> {
        self.compile_arg_pairs_for(b, "", args, None)
    }

    /// [`Compiler::compile_arg_pairs`], plus the by-reference argument check for
    /// a call whose callee is known by name.
    ///
    /// A named argument reaches a by-reference parameter just as a positional
    /// one does, so `sort(array: [1, 2])` is the same error as `sort([1, 2])`.
    /// Which slot it lands in is found by NAME rather than by position, which is
    /// the whole point of the syntax.
    /// The by-reference slots to judge a call to `callee` against: the USER
    /// function's own parameters when it is one, the library table otherwise. A
    /// name cannot be in both — redeclaring a library function is a fatal — so
    /// the two never have to be merged. The first half is the spelling the
    /// refusal names, which is the DECLARED one for a user function.
    fn byref_slots_for(&self, callee: &str, nargs: usize) -> ByRefSlots {
        match self.byref_user_slots(callee, nargs) {
            Some(slots) => slots,
            None => (
                callee.to_string(),
                byref_diag_slots(callee, nargs)
                    .into_iter()
                    .map(|(p, argno, param)| (p, argno, param.to_string()))
                    .collect(),
            ),
        }
    }

    /// [`Compiler::compile_arg_pairs`] for a method call. `method` is the
    /// temporary holding the receiver and the method name: each POSITIONAL
    /// argument written before the first spread or named one has a known
    /// position, and is judged against the method's by-reference parameters
    /// as it is sent (see [`ops::BYREF_ARG_DIAG_M`]).
    fn compile_arg_pairs_m(
        &mut self,
        b: &mut ChunkBuilder,
        args: &[Expr],
        method: Option<(&str, &Member)>,
    ) -> Result<(), String> {
        self.compile_arg_pairs_for(b, "", args, method)
    }

    fn compile_arg_pairs_for(
        &mut self,
        b: &mut ChunkBuilder,
        callee: &str,
        args: &[Expr],
        method: Option<(&str, &Member)>,
    ) -> Result<(), String> {
        let (shown, diag) = self.byref_slots_for(callee, args.len().max(BYREF_MAX_ARGNO));
        // A named argument that binds NOWHERE stops the judgement of every
        // argument written after it: the reference sends arguments in written
        // order and fails on the unknown name first, so `f(b: 2, a: 1)` is
        // `Unknown named parameter $b` even though `$a` is by reference. Only a
        // user function has its parameter names here; a library one keeps its
        // table-driven judgement, which the reference reaches the same way.
        let known = self
            .byref_user_fns
            .get(&callee.to_ascii_lowercase())
            .map(|f| {
                (
                    f.params
                        .iter()
                        .cloned()
                        .collect::<std::collections::HashSet<_>>(),
                    f.variadic,
                )
            });
        let mut blocked = false;
        // Positions stay exact until a spread or a named argument.
        let mut positional = true;
        for (i, a) in args.iter().enumerate() {
            if matches!(a, Expr::NamedArg(..) | Expr::Spread(_)) {
                positional = false;
            }
            let slot = match a {
                Expr::NamedArg(n, v) => {
                    let idx = b.add_constant(Value::str(n.clone()));
                    b.emit(Op::LoadConst(idx), 0);
                    // A name binds to the parameter it spells; on a VARIADIC
                    // by-reference callee it binds to the variadic tail
                    // instead, which has no name, so the slot is the one this
                    // argument's own position falls in.
                    let found = diag
                        .iter()
                        .find(|(_, _, param)| param == n)
                        .cloned()
                        .or_else(|| {
                            known.as_ref().and_then(|_| {
                                diag.iter()
                                    .find(|(p, _, param)| *p == i && param.is_empty())
                                    .map(|(p, _, param)| (*p, 1, param.clone()))
                            })
                        });
                    // A by-reference slot is an output location: an unset
                    // variable there is read quietly, as in the positional form.
                    match found {
                        Some(_) => self.compile_quiet(b, v)?,
                        None => self.compile_expr(b, v)?,
                    }
                    if let Some((names, variadic)) = &known {
                        if !variadic && !names.contains(n) {
                            blocked = true;
                        }
                    }
                    found
                }
                // `...$spread`: `true` in the name slot, flattened by the
                // host at the call. A spread contributes an unknown number of
                // arguments, so no by-reference diagnostic can be attached to
                // a position it might land in.
                Expr::Spread(inner) => {
                    b.emit(Op::LoadTrue, 0);
                    self.compile_expr(b, inner)?;
                    None
                }
                _ => {
                    b.emit(Op::LoadUndef, 0);
                    let found = diag.iter().find(|(p, ..)| *p == i).cloned();
                    match found {
                        Some(_) => self.compile_quiet(b, a)?,
                        None => self.compile_expr(b, a)?,
                    }
                    if let (true, Some((t, m))) = (positional, method) {
                        self.emit_byref_arg_diag_m(
                            b,
                            |c, b| {
                                c.emit_get_var(b, t);
                                Ok(())
                            },
                            m,
                            i,
                            a,
                        )?;
                    }
                    found
                }
            };
            if let Some((_, argno, param)) = slot.filter(|_| !blocked) {
                let inner = match a {
                    Expr::NamedArg(_, v) => v,
                    _ => a,
                };
                self.emit_byref_arg_diag(b, &shown, argno, &param, byref_arg_class(inner));
            }
        }
        Ok(())
    }

    /// Lower `f` as a chunk that runs in a DIFFERENT frame from the one being
    /// compiled — a method or closure body, a parameter default, a property or
    /// class-constant initializer, an enum case value.
    ///
    /// Such a chunk must address variables by name: the enclosing scope's slot
    /// numbers name its own frame's storage, and running them against another
    /// frame reads whatever happens to sit at that index there. (A `try` body is
    /// NOT one of these — it runs in the same frame and keeps the numbering.)
    fn in_other_frame<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        let saved = self.enter_scope(Vec::new());
        let r = f(self);
        self.leave_scope(saved);
        r
    }

    /// Begin lowering a scope whose variables are slot-addressed. Returns the
    /// previous scope's state, to be handed back to [`Compiler::leave_scope`] —
    /// a nested `function` declaration inside a function body must not inherit
    /// the outer frame's numbering.
    /// The locals of a scope that may be held in a fusevm frame slot.
    ///
    /// The by-reference table is handed to the analysis rather than rebuilt
    /// there: `collect_byref` has already recorded every user function's
    /// by-reference positions, and `seed_builtin_byref` the builtins', so the
    /// question "does this call take argument N by reference" already has one
    /// answer in this compiler.
    fn promotable_locals(
        &self,
        params: &[Param],
        body: &[Stmt],
        is_generator: bool,
    ) -> crate::promote::Promoted {
        crate::promote::promotable(params, body, is_generator, &|name, idx| {
            // Two tables, because a by-reference position can come from either.
            // `byref_positions` covers user functions and the builtins whose
            // out-parameters are written back; `byref_diag_slots` covers the
            // ones that take their argument by reference to MUTATE it — the
            // sort family, `array_push` and the rest of the array mutators,
            // which reach the variable by name and so must never be promoted.
            let written_back = self
                .byref_positions(name, idx + 1)
                .is_some_and(|(p, _)| p.contains(&idx));
            let mutated = byref_diag_slots(name, idx + 1)
                .iter()
                .any(|&(pos, ..)| pos == idx);
            // The array mutators are dispatched by name before either table is
            // consulted, so they are named here as well.
            let arrmut = array_mutator_subop(name).is_some() && idx == 0;
            written_back || mutated || arrmut
        })
    }

    fn enter_scope(&mut self, names: Vec<String>) -> SavedScope {
        self.enter_scope_promoting(
            names,
            crate::promote::Promoted {
                names: Vec::new(),
                numeric: FxHashSet::default(),
            },
        )
    }

    /// [`Compiler::enter_scope`], with the locals `promoted` held in frame slots
    /// instead of the host scope. They are removed from the host numbering, so
    /// each name lives in exactly one of the two spaces.
    fn enter_scope_promoting(
        &mut self,
        names: Vec<String>,
        promoted: crate::promote::Promoted,
    ) -> SavedScope {
        let names: Vec<String> = names
            .into_iter()
            .filter(|n| !promoted.names.iter().any(|p| p == n))
            .collect();
        let map = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.clone(), i as u32))
            .collect();
        (
            std::mem::replace(&mut self.slots, map),
            std::mem::replace(&mut self.slot_order, names),
            std::mem::replace(&mut self.fslots, crate::promote::slot_map(&promoted.names)),
            std::mem::replace(&mut self.fnumeric, promoted.numeric),
        )
    }

    /// Finish the scope, restore the enclosing one, and yield the slot order the
    /// finished scope settled on.
    fn leave_scope(&mut self, saved: SavedScope) -> Vec<String> {
        self.slots = saved.0;
        self.fslots = saved.2;
        self.fnumeric = saved.3;
        std::mem::replace(&mut self.slot_order, saved.1)
    }

    fn emit_get_var(&mut self, b: &mut ChunkBuilder, name: &str) {
        // A promoted local is the frame's own storage: one op, and one fusevm
        // can compile, where the host path needs a `CallBuiltin` it cannot.
        if let Some(&i) = self.fslots.get(name) {
            b.emit(Op::GetSlot(i), self.cur_line);
            return;
        }
        if let Some(&i) = self.slots.get(name) {
            b.emit(Op::LoadInt(i as i64), 0);
            b.emit(Op::CallBuiltin(ops::GETSLOT, 1), self.cur_line);
            return;
        }
        let idx = b.add_constant(Value::str(name.to_string()));
        b.emit(Op::LoadConst(idx), 0);
        b.emit(Op::CallBuiltin(ops::GETVAR, 1), self.cur_line);
    }

    /// Push the operand a variable write consumes: the slot index, or the name.
    /// Paired with [`Compiler::emit_store_var`], which must see the same choice.
    fn emit_var_target(&mut self, b: &mut ChunkBuilder, name: &str) {
        // `Op::SetSlot` carries its index in the op, so a promoted local needs
        // no target operand at all.
        if self.fslots.contains_key(name) {
            return;
        }
        match self.slots.get(name) {
            Some(&i) => b.emit(Op::LoadInt(i as i64), 0),
            None => {
                let idx = b.add_constant(Value::str(name.to_string()));
                b.emit(Op::LoadConst(idx), 0)
            }
        };
    }

    /// Store into the variable whose target [`Compiler::emit_var_target`]
    /// pushed, leaving the assigned value on the stack.
    fn emit_store_var(&mut self, b: &mut ChunkBuilder, name: &str) {
        // `Op::SetSlot` consumes the value and leaves nothing, while the two
        // builtins leave it — and every caller of this pair expects the value to
        // survive. Duplicating first keeps the stack effect identical.
        if let Some(&i) = self.fslots.get(name) {
            b.emit(Op::Dup, 0);
            b.emit(Op::SetSlot(i), self.cur_line);
            return;
        }
        let op = match self.slots.contains_key(name) {
            true => ops::SETSLOT,
            false => ops::SETVAR,
        };
        b.emit(Op::CallBuiltin(op, 2), 0);
    }

    /// The receiver of a property WRITE (`$recv->p = v`, `$recv->p[] = v`),
    /// fetched the way the reference fetches a write target's container.
    ///
    /// A property chain goes through [`ops::PROP_FETCH_W`], which creates a
    /// missing link as null instead of leaving it absent — so the write itself
    /// is what fails, with `Attempt to assign property "p" on null`. `rw` is
    /// the read-and-write fetch of a compound assignment or `++`: it still
    /// warns about the missing link (and an undefined variable), as a read
    /// does. A plain `=` warns about neither. Anything else is an ordinary read.
    fn compile_prop_write_recv(
        &mut self,
        b: &mut ChunkBuilder,
        recv: &Expr,
        rw: bool,
    ) -> Result<(), String> {
        match recv {
            // `$this` is fetched like any read: with no object bound that is
            // `Using $this when not in object context`, not a write on null.
            Expr::Var(n) if !rw && n != "this" => self.compile_quiet(b, recv),
            Expr::PropGet(inner, name) => {
                self.compile_prop_write_recv(b, inner, rw)?;
                self.emit_member(b, name, 0)?;
                b.emit(Op::LoadInt(i64::from(rw)), 0);
                b.emit(Op::CallBuiltin(ops::PROP_FETCH_W, 3), self.cur_line);
                Ok(())
            }
            _ => self.compile_expr(b, recv),
        }
    }

    /// Release the hidden temporary holding a `foreach` subject: read it, clear
    /// the slot, and hand the value to [`ops::GEN_RELEASE`], which destroys a
    /// suspended generator once no variable, element or property still holds it.
    fn emit_gen_release(
        &mut self,
        b: &mut ChunkBuilder,
        subj_t: &str,
        mark_t: &str,
    ) -> Result<(), String> {
        self.emit_get_var(b, subj_t);
        self.emit_get_var(b, mark_t);
        self.emit_set_var(b, subj_t, |_, b| {
            b.emit(Op::LoadUndef, 0);
            Ok(())
        })?;
        b.emit(Op::CallBuiltin(ops::GEN_RELEASE, 2), 0);
        b.emit(Op::Pop, 0);
        Ok(())
    }

    /// Emit `$name = <value produced by `f`>`, leaving the value on the stack.
    fn emit_set_var(
        &mut self,
        b: &mut ChunkBuilder,
        name: &str,
        f: impl FnOnce(&mut Self, &mut ChunkBuilder) -> Result<(), String>,
    ) -> Result<(), String> {
        self.emit_var_target(b, name);
        f(self, b)?;
        self.emit_store_var(b, name);
        // The desugared statements want no residual on the stack.
        b.emit(Op::Pop, 0);
        Ok(())
    }
}

/// Collect the variable names referenced anywhere in `e`, de-duplicated in
/// first-seen order — the free-variable set an arrow function captures by value.
/// Over-capturing (e.g. a name that is only ever assigned) is harmless because
/// capture is by value; a nested arrow fn contributes its body's free variables
/// minus its own parameters, and a nested `use(...)` closure contributes exactly
/// the names it captures.
/// Whether a call's argument list contains any named argument (`name: value`).
fn has_named(args: &[Expr]) -> bool {
    args.iter().any(|a| matches!(a, Expr::NamedArg(..)))
}

/// Whether an argument list needs the `(name, value)` pair encoding: a named
/// argument, or a `...$spread`.
///
/// A spread rides the same encoding, with a marker in the name slot (see
/// [`Compiler::compile_arg_pairs_for`]). Without it, `...` was accepted at ONE
/// call site — a call to a function named literally — and refused at every
/// other with the compile-time `'...' argument unpacking is only valid in a
/// function call`, so `$f(...$a)`, `$o->m(...$a)`, `C::s(...$a)` and
/// `new C(...$a)` were all hard failures rather than divergences.
/// The positions of the arguments written before the first spread or named
/// one — the only ones whose parameter position is known from the call site,
/// and so the only ones a run-time-resolved call can write a by-reference
/// result back to.
fn leading_positional(args: &[Expr]) -> Vec<usize> {
    args.iter()
        .take_while(|a| !matches!(a, Expr::NamedArg(..) | Expr::Spread(_)))
        .enumerate()
        .map(|(i, _)| i)
        .collect()
}

fn needs_arg_pairs(args: &[Expr]) -> bool {
    args.iter()
        .any(|a| matches!(a, Expr::NamedArg(..) | Expr::Spread(_)))
}

pub(crate) fn collect_free_vars(e: &Expr, out: &mut Vec<String>) {
    fn push(name: &str, out: &mut Vec<String>) {
        if !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    match e {
        Expr::Var(n) => push(n, out),
        // The name a variable variable reads is only known at run time, so
        // there is no name to capture here — but the operand that computes it
        // is an ordinary expression whose own free variables must be.
        Expr::VarVar(inner) => collect_free_vars(inner, out),
        Expr::Interp(parts) => {
            for p in parts {
                match p {
                    InterpPart::Expr(e) => collect_free_vars(e, out),
                    InterpPart::Lit(_) => {}
                }
            }
        }
        Expr::Array(elems, _) => {
            for e in elems {
                if let Some(k) = &e.key {
                    collect_free_vars(k, out);
                }
                collect_free_vars(&e.value, out);
            }
        }
        Expr::Index(a, b)
        | Expr::ListElem(a, b)
        | Expr::Binary(_, a, b)
        | Expr::InstanceOfDyn(a, b)
        | Expr::Elvis(a, b)
        | Expr::Coalesce(a, b) => {
            collect_free_vars(a, out);
            collect_free_vars(b, out);
        }
        Expr::Append(a)
        | Expr::Unary(_, a)
        | Expr::Spread(a)
        | Expr::Quiet(a)
        | Expr::Suppress(a)
        | Expr::IssetOf(a)
        | Expr::EmptyOf(a) => collect_free_vars(a, out),
        Expr::Assign(a, _, b) => {
            collect_free_vars(a, out);
            collect_free_vars(b, out);
        }
        Expr::IncDec { target, .. } => collect_free_vars(target, out),
        Expr::Call(_, args) => {
            for a in args {
                collect_free_vars(a, out);
            }
        }
        Expr::CallValue(callee, args) => {
            collect_free_vars(callee, out);
            for a in args {
                collect_free_vars(a, out);
            }
        }
        Expr::Ternary(a, b, c) => {
            collect_free_vars(a, out);
            collect_free_vars(b, out);
            collect_free_vars(c, out);
        }
        // The callable is an ordinary expression evaluated at the syntax, so the
        // variables it reads are free variables of whatever encloses it.
        Expr::Fcc { callable, .. } => collect_free_vars(callable, out),
        Expr::Match { subj, arms } => {
            collect_free_vars(subj, out);
            for arm in arms {
                if let Some(conds) = &arm.conds {
                    for c in conds {
                        collect_free_vars(c, out);
                    }
                }
                collect_free_vars(&arm.body, out);
            }
        }
        // A nested arrow fn captures its own body's free variables minus its
        // parameters; those free names must in turn be captured by the enclosing
        // arrow fn so the binding is available when the inner one runs.
        Expr::ArrowFn { params, body, .. } => {
            let mut inner = Vec::new();
            collect_free_vars(body, &mut inner);
            for n in inner {
                if !params.iter().any(|p| p.name == n) {
                    push(&n, out);
                }
            }
        }
        // A nested `use(...)` closure names the enclosing variables it captures.
        Expr::Closure { uses, .. } => {
            for u in uses {
                push(&u.name, out);
            }
        }
        // Only the CONSTRUCTOR arguments of an anonymous class are in the
        // enclosing scope; its body is a class body, which never reads one.
        Expr::New(_, args) | Expr::NewAnon { args, .. } => {
            for a in args {
                collect_free_vars(a, out);
            }
        }
        Expr::NewDyn(class, args) => {
            collect_free_vars(class, out);
            for a in args {
                collect_free_vars(a, out);
            }
        }
        Expr::StaticCall(class, _, args) => {
            if let ClassRef::Expr(c) = class {
                collect_free_vars(c, out);
            }
            for a in args {
                collect_free_vars(a, out);
            }
        }
        Expr::PropGet(recv, m) | Expr::NullsafePropGet(recv, m) => {
            collect_free_vars(recv, out);
            if let Some(d) = m.operand() {
                collect_free_vars(d, out);
            }
        }
        Expr::MethodCall(recv, m, args) | Expr::NullsafeMethodCall(recv, m, args) => {
            collect_free_vars(recv, out);
            if let Some(d) = m.operand() {
                collect_free_vars(d, out);
            }
            for a in args {
                collect_free_vars(a, out);
            }
        }
        Expr::NamedArg(_, v) => collect_free_vars(v, out),
        // A bareword class holds no variable, but `$cls::K` does — and an arrow
        // fn that says it must capture `$cls` along with everything else.
        Expr::StaticGet(class, _) | Expr::StaticProp(class, _) => {
            if let ClassRef::Expr(c) = class {
                collect_free_vars(c, out);
            }
        }
        Expr::Throw(inner) | Expr::Clone(inner) => collect_free_vars(inner, out),
        // A magic constant closes over nothing: every part of it is either a
        // compile-time literal or read from the host.
        Expr::ConstFetch(_) | Expr::Magic(_) => {}
        Expr::Unset(targets) => {
            for t in targets {
                collect_free_vars(t, out);
            }
        }
        Expr::InstanceOf(e, _) => collect_free_vars(e, out),
        Expr::RefAssign(a, b) => {
            collect_free_vars(a, out);
            collect_free_vars(b, out);
        }
        Expr::Yield { key, value } => {
            if let Some(k) = key {
                collect_free_vars(k, out);
            }
            if let Some(v) = value {
                collect_free_vars(v, out);
            }
        }
        Expr::YieldFrom(src) => collect_free_vars(src, out),
        Expr::Print(a) => collect_free_vars(a, out),
        Expr::Include(_, a) | Expr::Eval(a) => collect_free_vars(a, out),
        Expr::Null | Expr::Hole | Expr::Bool(_) | Expr::Int(_) | Expr::Float(_) | Expr::Str(_) => {}
    }
}

/// Whether a function/method/closure body contains a top-level `yield` (making it a
/// generator). The walk stops at nested function/closure/arrow-function boundaries:
/// a `yield` inside a nested closure belongs to *that* closure, not the enclosing
/// function.
fn body_has_yield(body: &[Stmt]) -> bool {
    body.iter().any(stmt_has_yield)
}

fn stmt_has_yield(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::Expr(e) | StmtKind::Return(Some(e)) => expr_has_yield(e),
        StmtKind::Echo(es) => es.iter().any(expr_has_yield),
        StmtKind::If {
            cond,
            then,
            elifs,
            els,
        } => {
            expr_has_yield(cond)
                || body_has_yield(then)
                || elifs
                    .iter()
                    .any(|(c, b)| expr_has_yield(c) || body_has_yield(b))
                || els.as_ref().is_some_and(|b| body_has_yield(b))
        }
        StmtKind::While { cond, body } | StmtKind::DoWhile { cond, body } => {
            expr_has_yield(cond) || body_has_yield(body)
        }
        StmtKind::For {
            init,
            cond,
            step,
            body,
        } => {
            init.iter().any(expr_has_yield)
                || cond.as_ref().is_some_and(expr_has_yield)
                || step.iter().any(expr_has_yield)
                || body_has_yield(body)
        }
        StmtKind::Foreach { arr, body, .. } => expr_has_yield(arr) || body_has_yield(body),
        StmtKind::Switch { subj, cases } => {
            expr_has_yield(subj)
                || cases
                    .iter()
                    .any(|c| c.test.as_ref().is_some_and(expr_has_yield) || body_has_yield(&c.body))
        }
        StmtKind::Try {
            body,
            catches,
            finally,
        } => {
            body_has_yield(body)
                || catches.iter().any(|c| body_has_yield(&c.body))
                || finally.as_ref().is_some_and(|b| body_has_yield(b))
        }
        StmtKind::Block(b) => body_has_yield(b),
        // Nested declarations own their own yields; a `return;` / `break` / etc.
        // carry none.
        _ => false,
    }
}

fn expr_has_yield(e: &Expr) -> bool {
    match e {
        Expr::Yield { .. } | Expr::YieldFrom(_) => true,
        Expr::Unary(_, a)
        | Expr::Spread(a)
        | Expr::Index(a, _)
        | Expr::Append(a)
        | Expr::Throw(a)
        | Expr::Clone(a)
        | Expr::Print(a)
        | Expr::Include(_, a)
        | Expr::Eval(a)
        | Expr::InstanceOf(a, _)
        | Expr::NamedArg(_, a) => expr_has_yield(a),
        Expr::Binary(_, a, b)
        | Expr::InstanceOfDyn(a, b)
        | Expr::Elvis(a, b)
        | Expr::Coalesce(a, b)
        | Expr::RefAssign(a, b) => expr_has_yield(a) || expr_has_yield(b),
        Expr::Assign(a, _, b) => expr_has_yield(a) || expr_has_yield(b),
        Expr::Ternary(a, c, d) => expr_has_yield(a) || expr_has_yield(c) || expr_has_yield(d),
        Expr::IncDec { target, .. } => expr_has_yield(target),
        Expr::Call(_, args) | Expr::New(_, args) | Expr::NewAnon { args, .. } => {
            args.iter().any(expr_has_yield)
        }
        Expr::NewDyn(class, args) => expr_has_yield(class) || args.iter().any(expr_has_yield),
        Expr::StaticCall(class, _, args) => {
            class.operand().is_some_and(expr_has_yield) || args.iter().any(expr_has_yield)
        }
        Expr::StaticGet(class, _) | Expr::StaticProp(class, _) => {
            class.operand().is_some_and(expr_has_yield)
        }
        Expr::CallValue(c, args) => expr_has_yield(c) || args.iter().any(expr_has_yield),
        Expr::PropGet(r, m) | Expr::NullsafePropGet(r, m) => {
            expr_has_yield(r) || m.operand().is_some_and(expr_has_yield)
        }
        Expr::MethodCall(r, m, args) | Expr::NullsafeMethodCall(r, m, args) => {
            expr_has_yield(r)
                || m.operand().is_some_and(expr_has_yield)
                || args.iter().any(expr_has_yield)
        }
        Expr::Array(items, _) => items
            .iter()
            .any(|e| e.key.as_ref().is_some_and(expr_has_yield) || expr_has_yield(&e.value)),
        // `Interp` parts are only literals and bare `$var`s — neither holds a yield.
        Expr::Interp(parts) => parts.iter().any(|p| match p {
            InterpPart::Expr(e) => expr_has_yield(e),
            InterpPart::Lit(_) => false,
        }),
        Expr::Match { subj, arms } => {
            expr_has_yield(subj)
                || arms.iter().any(|a| {
                    a.conds
                        .as_ref()
                        .is_some_and(|cs| cs.iter().any(expr_has_yield))
                        || expr_has_yield(&a.body)
                })
        }
        Expr::Unset(targets) => targets.iter().any(expr_has_yield),
        // Nested function definitions own their own yields.
        _ => false,
    }
}

/// PHP's compile-time error for a `break`/`continue` level that exceeds the
/// number of enclosing loops, or that appears outside a loop entirely.
fn break_level_error(kw: &str, level: u32, depth: usize) -> String {
    if level == 0 {
        format!("'{kw}' operator accepts only positive integers")
    } else if depth == 0 {
        format!("'{kw}' not in the 'loop' or 'switch' context")
    } else {
        format!("Cannot '{kw}' {level} levels")
    }
}

// ── the `*` operand swap ─────────────────────────────────────────────────────
//
// The reference's compiler puts a constant operand of `*` in the second slot,
// which shows up in `Unsupported operand types: X * Y` and in which operand is
// coerced (and so warns) first. Reproducing it needs the same notion of
// "constant" the reference's own folder uses, and the two predicates below
// deliberately answer a NARROWER question than that in the safe direction.
//
// Being wrong costs nothing in one direction and a divergence in the other. If
// something constant is called runtime, the swap happens where the reference did
// not do one — a NEW divergence. If something runtime is called constant, the
// swap is skipped and the old answer stands. So `is_definitely_runtime` returns
// true only for forms that cannot possibly be folded, and everything it is
// unsure about is treated as constant.
//
// A function call is exactly why that matters: `"g" * strlen("ab")` does NOT
// swap in the reference, because it folds `strlen()` on a literal argument at
// compile time. Calls are therefore not "definitely runtime" here.

/// Whether `e` is a constant the reference would hold in an `IS_CONST` operand.
///
/// Literals, and the arithmetic over literals that folds without a diagnostic.
fn is_const_operand(e: &Expr) -> bool {
    match e {
        Expr::Null | Expr::Bool(_) | Expr::Int(_) | Expr::Float(_) | Expr::Str(_) => true,
        // A double-quoted string with no embedded expression is a literal; the
        // lexer does not collapse it, so `"g"` arrives as a one-part `Interp`.
        Expr::Interp(parts) => parts.iter().all(|p| matches!(p, InterpPart::Lit(_))),
        Expr::Unary(UnOp::Neg | UnOp::Pos, x) => is_const_operand(x),
        Expr::Binary(op, a, b) if is_foldable_arith(*op) => folds_without_diagnostic(*op, a, b),
        _ => false,
    }
}

/// Whether every element of an array literal is a compile-time constant, the
/// condition for the reference to build the array while compiling.
fn is_const_array(elems: &[ArrayElem]) -> bool {
    let is_const = |e: &Expr| match e {
        Expr::Array(inner, _) => is_const_array(inner),
        Expr::Spread(x) => {
            matches!(&**x, Expr::Array(inner, _) if is_const_array(inner)) || is_const_operand(x)
        }
        _ => is_const_operand(e),
    };
    elems
        .iter()
        .all(|e| !e.by_ref && e.key.as_ref().map_or(true, is_const_operand) && is_const(&e.value))
}

/// The type name of the first scalar a CONSTANT array literal unpacks with
/// `...`, which the reference refuses at compile time. `None` when the literal
/// is not constant, or every spread in it is an array.
fn const_array_bad_spread(elems: &[ArrayElem]) -> Option<&'static str> {
    if !is_const_array(elems) {
        return None;
    }
    elems.iter().find_map(|e| match &e.value {
        Expr::Spread(x) => literal_type_name(x),
        _ => None,
    })
}

/// The diagnostic name of a scalar literal (`int`, `string`, and `true` or
/// `false` for a bool, as the reference spells it); `None` for anything whose
/// type is not evident from its spelling.
fn literal_type_name(e: &Expr) -> Option<&'static str> {
    match e {
        Expr::Null => Some("null"),
        // `zend_zval_value_name`: a bool is named by its value.
        Expr::Bool(true) => Some("true"),
        Expr::Bool(false) => Some("false"),
        Expr::Int(_) => Some("int"),
        Expr::Float(_) => Some("float"),
        Expr::Str(_) | Expr::Interp(_) => Some("string"),
        Expr::Unary(UnOp::Neg | UnOp::Pos, x) => match literal_type_name(x)? {
            t @ ("int" | "float") => Some(t),
            _ => None,
        },
        _ => None,
    }
}

/// Whether `e` cannot be a compile-time constant under ANY folding rule, so a
/// swap against it is certainly what the reference did.
fn is_definitely_runtime(e: &Expr) -> bool {
    match e {
        Expr::Var(_)
        | Expr::Index(..)
        | Expr::Append(_)
        | Expr::Assign(..)
        | Expr::IncDec { .. }
        | Expr::RefAssign(..) => true,
        // Arithmetic over constants that the reference would NOT fold, because
        // folding it would have to emit the diagnostic at compile time.
        Expr::Binary(op, a, b) if is_foldable_arith(*op) => {
            is_const_operand(a) && is_const_operand(b) && !folds_without_diagnostic(*op, a, b)
        }
        // A bitwise operator over a float constant with a fractional part
        // deprecates the lossy conversion, so the reference does not fold it.
        Expr::Binary(BinOp::BitOr | BinOp::BitAnd | BinOp::BitXor, a, b) => {
            is_const_operand(a)
                && is_const_operand(b)
                && (is_fractional_float(a) || is_fractional_float(b))
        }
        _ => false,
    }
}

/// The arithmetic operators whose constant folding this models. `Concat` and the
/// comparisons fold too, but they never warn on constants, so they are always
/// constant and need no separate test.
fn is_foldable_arith(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod | BinOp::Pow
    )
}

/// Whether `a <op> b` on two constants evaluates silently, which is the
/// condition for the reference to fold it.
///
/// Arithmetic on a string constant that is only LEADING-numeric warns
/// (`"0x1A" * 1`) and on a wholly non-numeric one throws, so neither folds. A
/// division or modulo by a zero constant does not fold either.
fn folds_without_diagnostic(op: BinOp, a: &Expr, b: &Expr) -> bool {
    if !is_silently_numeric(a) || !is_silently_numeric(b) {
        return false;
    }
    if matches!(op, BinOp::Div | BinOp::Mod) && is_literal_zero(b) {
        return false;
    }
    true
}

/// Whether `e` is a constant with a numeric reading that costs no diagnostic.
fn is_silently_numeric(e: &Expr) -> bool {
    match e {
        Expr::Null | Expr::Bool(_) | Expr::Int(_) | Expr::Float(_) => true,
        Expr::Str(s) => is_fully_numeric(s),
        Expr::Interp(parts) => match parts.as_slice() {
            [] => is_fully_numeric(""),
            [InterpPart::Lit(s)] => is_fully_numeric(s),
            _ => false,
        },
        Expr::Unary(UnOp::Neg | UnOp::Pos, x) => is_silently_numeric(x),
        Expr::Binary(op, a, b) if is_foldable_arith(*op) => folds_without_diagnostic(*op, a, b),
        _ => false,
    }
}

/// Whether an expression is a literal value written in the source.
///
/// Distinguishes the two ways a destructuring subject can fail to be
/// referenceable, which PHP treats differently: a literal is a fatal
/// (`Cannot assign reference to non referenceable value`), while a temporary
/// that merely has no home — a call result — is a notice and then a copy.
fn is_literal(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Null
            | Expr::Bool(_)
            | Expr::Int(_)
            | Expr::Float(_)
            | Expr::Str(_)
            | Expr::Interp(_)
            | Expr::Array(..)
    )
}

/// Whether `e` is a float literal (possibly negated) with a fractional part —
/// one an integer operator cannot take without a precision-loss deprecation.
fn is_fractional_float(e: &Expr) -> bool {
    match e {
        Expr::Float(f) => f.is_finite() && f.fract() != 0.0,
        Expr::Unary(UnOp::Neg | UnOp::Pos, x) => is_fractional_float(x),
        _ => false,
    }
}

fn is_literal_zero(e: &Expr) -> bool {
    match e {
        Expr::Int(0) => true,
        Expr::Float(f) => *f == 0.0,
        Expr::Str(s) => {
            is_fully_numeric(s) && s.trim().parse::<f64>().map(|v| v == 0.0) == Ok(true)
        }
        _ => false,
    }
}

/// PHP's "numeric string": optional surrounding whitespace around a full
/// integer or float. A string that merely STARTS with a number ("5g", "0x1A")
/// is not one — it is the leading-numeric case that warns.
fn is_fully_numeric(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    // `parse::<f64>` accepts forms PHP does not ("inf", "NaN", "1e", hex).
    let bytes = t.as_bytes();
    let mut i = 0;
    if matches!(bytes[i], b'+' | b'-') {
        i += 1;
    }
    let digits_before = {
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        i - start
    };
    let mut digits_after = 0;
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        digits_after = i - start;
    }
    if digits_before + digits_after == 0 {
        return false;
    }
    if i < bytes.len() && matches!(bytes[i], b'e' | b'E') {
        i += 1;
        if i < bytes.len() && matches!(bytes[i], b'+' | b'-') {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    i == bytes.len()
}

// ── slot resolution ──────────────────────────────────────────────────────────
//
// A scope's variables are numbered so the chunk can address them by index. The
// index and the name reach the SAME storage (`host::Vars`), so `extract()`,
// `$$name`, `unset()`, a reference binding, an array-path write and a
// by-reference parameter's write-back all remain coherent with slot access —
// they simply arrive at the slot by name instead of by number. That is what
// keeps this list of exclusions short: it needs to cover only the names whose
// *slot number* would be wrong, not every name something else might touch.

/// Names that must keep the by-name path in every scope.
///
/// A superglobal resolves to the global frame from wherever it is read, so a
/// slot number in the current frame would address the wrong storage. `this` is
/// bound by the call machinery, not by the body.
/// Whether `e` can be proved, from its shape alone, never to evaluate to an
/// array — in which case an assignment of it needs no copy.
///
/// Only `+` among the operators can produce one, and only when BOTH operands
/// are arrays (`[1] + [2]` is array union). Every other arithmetic operator on
/// two arrays is a `TypeError` rather than an array, so its result is a scalar
/// whenever it is a value at all. Saying "no" is always safe: it just keeps the
/// copy.
fn never_array(e: &Expr) -> bool {
    match e {
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Null => true,
        // An interpolated string is a string, and a comparison is a bool.
        Expr::Interp(_) => true,
        Expr::Binary(op, l, r) => match op {
            BinOp::Add => never_array(l) || never_array(r),
            BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Mod
            | BinOp::Pow
            | BinOp::Concat
            | BinOp::BitAnd
            | BinOp::BitOr
            | BinOp::BitXor
            | BinOp::Shl
            | BinOp::Shr
            | BinOp::Lt
            | BinOp::Gt
            | BinOp::Le
            | BinOp::Ge
            | BinOp::LooseEq
            | BinOp::LooseNe
            | BinOp::StrictEq
            | BinOp::StrictNe
            | BinOp::Spaceship => true,
            _ => false,
        },
        Expr::IncDec { .. }
        | Expr::InstanceOf(..)
        | Expr::InstanceOfDyn(..)
        | Expr::IssetOf(_)
        | Expr::EmptyOf(_) => true,
        Expr::Unary(op, x) => match op {
            // `!` is a bool; `-`/`+` are numbers; `~` is an int or string.
            UnOp::Not => true,
            UnOp::Neg | UnOp::Pos | UnOp::BitNot => never_array(x),
        },
        _ => false,
    }
}

fn slottable_name(name: &str) -> bool {
    !crate::host::is_superglobal(name) && name != "this" && !name.starts_with('@')
}

/// Collect the variables a scope may address by slot: every name it mentions,
/// minus the [`slottable_name`] exclusions.
///
/// Missing a name is safe — it simply keeps the by-name path, which reaches the
/// same storage — so this walks the forms PHP code is actually written in
/// rather than exhaustively. It does NOT descend into a nested scope (a
/// closure, an arrow function, a nested `function` or `class` body): those are
/// compiled against their own frame and number their own slots.
fn scope_slots(params: &[Param], body: &[Stmt]) -> Vec<String> {
    let mut c = SlotScan {
        out: Vec::new(),
        seen: FxHashSet::default(),
    };
    for p in params {
        c.push(&p.name);
    }
    c.stmts(body);
    c.out
}

struct SlotScan {
    out: Vec<String>,
    seen: FxHashSet<String>,
}

impl SlotScan {
    fn push(&mut self, n: &str) {
        if slottable_name(n) && self.seen.insert(n.to_string()) {
            self.out.push(n.to_string());
        }
    }

    fn stmts(&mut self, body: &[Stmt]) {
        for s in body {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Echo(es) => self.exprs(es),
            StmtKind::Expr(e) => self.expr(e),
            StmtKind::Return(Some(e)) => self.expr(e),
            StmtKind::If {
                cond,
                then,
                elifs,
                els,
            } => {
                self.expr(cond);
                self.stmts(then);
                for (c, b) in elifs {
                    self.expr(c);
                    self.stmts(b);
                }
                if let Some(b) = els {
                    self.stmts(b);
                }
            }
            StmtKind::While { cond, body } | StmtKind::DoWhile { cond, body } => {
                self.expr(cond);
                self.stmts(body);
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                self.exprs(init);
                if let Some(c) = cond {
                    self.expr(c);
                }
                self.exprs(step);
                self.stmts(body);
            }
            StmtKind::Foreach {
                arr,
                key_var,
                val,
                body,
                ..
            } => {
                self.expr(arr);
                if let Some(k) = key_var {
                    self.push(k);
                }
                match val {
                    ForeachVal::Var(n) => self.push(n),
                    ForeachVal::Pattern(e) => self.expr(e),
                }
                self.stmts(body);
            }
            StmtKind::Switch { subj, cases } => {
                self.expr(subj);
                for c in cases {
                    if let Some(e) = &c.test {
                        self.expr(e);
                    }
                    self.stmts(&c.body);
                }
            }
            StmtKind::Try {
                body,
                catches,
                finally,
            } => {
                self.stmts(body);
                for c in catches {
                    if let Some(v) = &c.var {
                        self.push(v);
                    }
                    self.stmts(&c.body);
                }
                if let Some(f) = finally {
                    self.stmts(f);
                }
            }
            StmtKind::Block(b) => self.stmts(b),
            // The name is bound in THIS frame (as an alias), so it needs a slot
            // here just as an ordinary local would.
            StmtKind::Global(names) => {
                for n in names {
                    self.push(n);
                }
            }
            StmtKind::StaticLocal(decls) => {
                for (n, init) in decls {
                    self.push(n);
                    if let Some(e) = init {
                        self.expr(e);
                    }
                }
            }
            StmtKind::ConstDecl(ds) => {
                for (_, e) in ds {
                    self.expr(e);
                }
            }
            // A nested scope numbers its own slots; a declaration binds no
            // variable in this one.
            StmtKind::Function { .. }
            | StmtKind::Class(_)
            | StmtKind::InlineHtml(_)
            | StmtKind::Return(None)
            | StmtKind::Break(_)
            | StmtKind::Goto(_)
            | StmtKind::Label(_)
            | StmtKind::Continue(_) => {}
        }
    }

    fn exprs(&mut self, es: &[Expr]) {
        for e in es {
            self.expr(e);
        }
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Var(n) => self.push(n),
            Expr::Interp(parts) => {
                for p in parts {
                    if let InterpPart::Expr(e) = p {
                        self.expr(e);
                    }
                }
            }
            Expr::Array(elems, _) => {
                for el in elems {
                    if let Some(k) = &el.key {
                        self.expr(k);
                    }
                    self.expr(&el.value);
                }
            }
            Expr::Index(a, b)
            | Expr::ListElem(a, b)
            | Expr::Binary(_, a, b)
            | Expr::InstanceOfDyn(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Append(a)
            | Expr::Unary(_, a)
            | Expr::Spread(a)
            | Expr::Clone(a)
            | Expr::Throw(a)
            | Expr::Quiet(a)
            | Expr::Print(a)
            | Expr::Include(_, a)
            | Expr::Eval(a)
            | Expr::NamedArg(_, a)
            | Expr::InstanceOf(a, _) => self.expr(a),
            Expr::PropGet(a, m) | Expr::NullsafePropGet(a, m) => {
                self.expr(a);
                if let Some(d) = m.operand() {
                    self.expr(d);
                }
            }
            Expr::Assign(a, _, b) | Expr::RefAssign(a, b) | Expr::Elvis(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Coalesce(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::IncDec { target, .. } => self.expr(target),
            Expr::Call(_, args) | Expr::New(_, args) | Expr::NewAnon { args, .. } => {
                self.exprs(args)
            }
            Expr::CallValue(f, args) => {
                self.expr(f);
                self.exprs(args);
            }
            Expr::NewDyn(class, args) => {
                self.expr(class);
                self.exprs(args);
            }
            Expr::MethodCall(r, m, args) | Expr::NullsafeMethodCall(r, m, args) => {
                self.expr(r);
                if let Some(d) = m.operand() {
                    self.expr(d);
                }
                self.exprs(args);
            }
            Expr::StaticCall(_, _, args) => self.exprs(args),
            Expr::Ternary(a, b, c) => {
                self.expr(a);
                self.expr(b);
                self.expr(c);
            }
            Expr::Match { subj, arms } => {
                self.expr(subj);
                for a in arms {
                    if let Some(cs) = &a.conds {
                        self.exprs(cs);
                    }
                    self.expr(&a.body);
                }
            }
            Expr::Unset(es) => self.exprs(es),
            // A closure/arrow body is its own scope; its `use (...)` names,
            // however, are read out of THIS one.
            Expr::Closure { uses, .. } => {
                for u in uses {
                    self.push(&u.name);
                }
            }
            _ => {}
        }
    }
}

/// Whether `e` already evaluates to a `Bool`, so PHP's truthiness coercion would
/// return it unchanged. Deliberately narrow: every operator listed here answers
/// with `Value::Bool` on every input, with no coercion of its own to apply.
fn yields_bool(e: &Expr) -> bool {
    match e {
        Expr::Bool(_) => true,
        Expr::InstanceOf(..) => true,
        Expr::InstanceOfDyn(..) => true,
        Expr::Unary(UnOp::Not, _) => true,
        Expr::Binary(op, ..) => matches!(
            op,
            BinOp::Lt
                | BinOp::Gt
                | BinOp::Le
                | BinOp::Ge
                | BinOp::LooseEq
                | BinOp::LooseNe
                | BinOp::StrictEq
                | BinOp::StrictNe
        ),
        _ => false,
    }
}

/// `zend_can_write_to_variable`: whether a destructuring target is a place a
/// value can be stored. Subscripts and property fetches are peeled down to
/// their base, which must be a variable or a call (a call base is writable
/// through its result, `f()[0]`), and a `?->` anywhere on that spine makes the
/// whole target read-only.
fn list_target_writable(target: &Expr) -> bool {
    let mut cur = target;
    loop {
        match cur {
            Expr::Index(r, _) | Expr::Append(r) | Expr::PropGet(r, _) => cur = r,
            Expr::NullsafePropGet(..) | Expr::NullsafeMethodCall(..) => return false,
            Expr::MethodCall(r, _, _) if chain_has_nullsafe(r) => return false,
            Expr::Var(_)
            | Expr::VarVar(_)
            | Expr::StaticProp(..)
            | Expr::Call(..)
            | Expr::CallValue(..)
            | Expr::MethodCall(..)
            | Expr::StaticCall(..) => return true,
            _ => return false,
        }
    }
}

/// `Type $p = null` whose type does not already admit null — the pre-`?T`
/// spelling of a nullable parameter. The reference widens the declared type
/// with `null`, so an explicit `null` argument is accepted too.
fn implicitly_nullable(p: &Param) -> bool {
    matches!(p.default, Some(Expr::Null))
        && p.ty.as_ref().is_some_and(|t| {
            !t.nullable() && !t.parts.iter().any(|x| x.eq_ignore_ascii_case("mixed"))
        })
}

/// `zend_is_auto_global`: the superglobals, which a parameter or a `use`
/// clause may not rebind. `$argv`/`$argc` are ordinary globals and do not count.
fn is_auto_global(name: &str) -> bool {
    crate::host::is_superglobal(name) && name != "argv" && name != "argc"
}

/// The return rule a body's declared type imposes on its `return`s: `void`,
/// `never`, or none. A generator's declared type describes the `Generator`, not
/// what its `return` statements carry, so it imposes none.
fn ret_rule(ret: Option<&TypeHint>, body: &[Stmt]) -> Option<&'static str> {
    let [only] = ret?.parts.as_slice() else {
        return None;
    };
    if body_has_yield(body) {
        return None;
    }
    if only.eq_ignore_ascii_case("void") {
        Some("void")
    } else if only.eq_ignore_ascii_case("never") {
        Some("never")
    } else {
        None
    }
}

/// A `goto` label: where it stands, in which chunk, and inside which loops.
#[derive(Clone)]
struct LabelAt {
    pos: usize,
    chunk: usize,
    loops: Vec<usize>,
}

/// A `goto` whose jump waits for its label.
#[derive(Clone)]
struct PendingGoto {
    label: String,
    jump: usize,
    chunk: usize,
    loops: Vec<usize>,
    line: u32,
}

/// The labels and jumps of one function body (labels are function-scoped).
#[derive(Clone, Default)]
struct Gotos {
    labels: FxHashMap<String, LabelAt>,
    pending: Vec<PendingGoto>,
}

/// What a function, method or closure body hides from its surroundings —
/// see [`Compiler::enter_own_loop_scope`].
struct OwnScope {
    in_try: usize,
    outer_loops: Vec<bool>,
    gotos: Gotos,
    chunk: usize,
}
