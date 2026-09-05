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

/// Why a declaration the compiler could read cannot be LINKED, and in which of
/// the reference's two shapes it says so.
enum LinkError {
    /// A bare fatal error: displayed with a stack trace, but raised below the
    /// exception machinery, so no `try`/`catch` can see it.
    Fatal(String),
    /// The message of an ordinary throwable `Error`.
    Throw(String),
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
}

/// Break/continue jump fixups for the innermost loop.
struct LoopCtx {
    breaks: Vec<usize>,
    continues: Vec<usize>,
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
    ("preg_replace_callback", 5, "count"),
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
    /// Emit per-statement DAP line markers (`php --dap`). Off for normal runs so
    /// the compiled chunk carries zero extra ops.
    debug: bool,
    /// Set while lowering the body of a `function &f()`, so a `return` naming an
    /// lvalue publishes that storage cell instead of copying its value.
    ret_by_ref: bool,
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
}

/// Compile a parsed program. `debug` enables per-statement DAP line markers.
pub fn compile(stmts: &[Stmt], debug: bool) -> Result<Program, String> {
    let mut c = Compiler {
        debug,
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
    let saved = c.enter_scope_promoting(scope_slots(&[], stmts), promoted);
    c.compile_seq(&mut b, stmts)?;
    let main_locals = c.leave_scope(saved);
    Ok(Program {
        main: b.build(),
        main_locals,
        functions: c.functions,
        classes: c.classes,
        try_defs: c.try_defs,
        // Drained here rather than in the lexer's caller: this is the last point
        // that still belongs to compiling THIS source, so no later compilation
        // (the prelude, an `eval`) can inherit or lose them.
        diags: crate::lexer::take_diags(),
    })
}

impl Compiler {
    fn tmp_name(&mut self, tag: &str) -> String {
        self.tmp += 1;
        format!("@{tag}{}", self.tmp)
    }

    /// Compile a formal parameter list, lowering each default-value expression to
    /// its own chunk (run in the callee frame when the argument is omitted).
    /// Shared by named functions, closures, and methods.
    fn compile_params(&mut self, params: &[Param]) -> Result<Vec<host::Param>, String> {
        let mut out = Vec::with_capacity(params.len());
        for p in params {
            let default = match &p.default {
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
                ty: p.ty.clone(),
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

    fn emit_byref_writeback(
        &mut self,
        b: &mut ChunkBuilder,
        args: &[Expr],
        positions: &[usize],
        guarded: bool,
    ) -> Result<(), String> {
        for &pos in positions {
            let Some(arg) = args.get(pos) else { continue };
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
                    let back = Expr::Assign(Box::new(arg.clone()), None, Box::new(Expr::Var(tmp)));
                    self.compile_expr(b, &back)?;
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
            ("preg_replace_callback", &[4]),
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

    /// Pre-pass: record the by-reference parameter positions of every `function`
    /// declaration (recursing into nested bodies) so call sites can write the
    /// callee's finals back to the caller — even for forward references.
    /// Index into `self.loops` of the loop a `break`/`continue` of `level`
    /// targets — level 1 is the innermost — or `None` when the chunk does not
    /// have that many enclosing loops.
    fn loop_at_level(&self, level: u32) -> Option<usize> {
        self.loops.len().checked_sub(level.max(1) as usize)
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

    fn compile_seq(&mut self, b: &mut ChunkBuilder, body: &[Stmt]) -> Result<(), String> {
        for s in body {
            self.compile_stmt(b, s)?;
        }
        Ok(())
    }

    fn compile_stmt(&mut self, b: &mut ChunkBuilder, s: &Stmt) -> Result<(), String> {
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
            StmtKind::Expr(e) => {
                self.compile_expr(b, e)?;
                b.emit(Op::Pop, line);
            }
            StmtKind::Block(body) => self.compile_seq(b, body)?,
            StmtKind::Global(names) => {
                for name in names {
                    let nidx = b.add_constant(Value::str(name.clone()));
                    b.emit(Op::LoadConst(nidx), line);
                    b.emit(Op::CallBuiltin(ops::GLOBAL_BIND, 1), line);
                    b.emit(Op::Pop, 0);
                }
            }
            StmtKind::StaticLocal(decls) => {
                for (name, default) in decls {
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
                b.emit(Op::CallBuiltin(ops::SIG_RETURN, 1), line);
                b.emit(Op::Pop, line);
            }
            StmtKind::Break(level) => {
                // `break n` leaves the n-th enclosing loop, so index the loop
                // stack from the top: level 1 is the innermost. Inside a loop in
                // this chunk → an in-chunk jump. Inside a `try` body with no such
                // loop → a control signal the orchestrator relays to the
                // enclosing loop.
                if let Some(idx) = self.loop_at_level(*level) {
                    let j = b.emit(Op::Jump(0), line);
                    self.loops[idx].breaks.push(j);
                } else if self.in_try > 0 {
                    // No loop for it in this chunk: raise a signal carrying the
                    // levels still to unwind, which the `try` dispatch in the
                    // enclosing chunk resolves (or re-raises, decremented).
                    b.emit(Op::LoadInt(*level as i64), line);
                    b.emit(Op::CallBuiltin(ops::SIG_BREAK, 1), line);
                    b.emit(Op::Pop, line);
                } else {
                    return Err(break_level_error("break", *level, self.loops.len()));
                }
            }
            StmtKind::Continue(level) => {
                if let Some(idx) = self.loop_at_level(*level) {
                    let j = b.emit(Op::Jump(0), line);
                    self.loops[idx].continues.push(j);
                } else if self.in_try > 0 {
                    b.emit(Op::LoadInt(*level as i64), line);
                    b.emit(Op::CallBuiltin(ops::SIG_CONTINUE, 1), line);
                    b.emit(Op::Pop, line);
                } else {
                    return Err(break_level_error("continue", *level, self.loops.len()));
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
            } => {
                // Each default-value expression is lowered to its own tiny chunk,
                // run in the callee frame when the argument is omitted (host).
                let cparams = self.compile_params(params)?;
                let mut fb = ChunkBuilder::new();
                // A function body has its own loop scope: a break inside it must
                // not target a loop at the call site.
                let saved = std::mem::take(&mut self.loops);
                let saved_ref = std::mem::replace(&mut self.ret_by_ref, *by_ref_return);
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
                let locals = self.leave_scope(locals);
                self.decl_site = saved_site;
                self.ret_by_ref = saved_ref;
                self.loops = saved;
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
                    },
                ));
            }
            StmtKind::Class(decl) => self.compile_class(b, decl)?,
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
        self.loops.push(LoopCtx {
            breaks: vec![],
            continues: vec![],
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
        self.loops.push(LoopCtx {
            breaks: vec![],
            continues: vec![],
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
        self.loops.push(LoopCtx {
            breaks: vec![],
            continues: vec![],
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
        self.loops.push(LoopCtx {
            breaks: vec![],
            continues: vec![],
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
        let (val_name, pattern) = match val {
            ForeachVal::Var(n) => (n.clone(), None),
            ForeachVal::Pattern(p) => (self.tmp_name("fev"), Some(p)),
        };
        let val_var: &str = &val_name;

        // Evaluate the subject once into a hidden temporary, then branch: a lazy
        // generator loop, or the array/iterator index loop.
        let subj_t = self.tmp_name("subj");
        self.emit_set_var(b, &subj_t, |c, b| c.compile_expr(b, arr))?;

        self.emit_get_var(b, &subj_t);
        b.emit(Op::CallBuiltin(ops::IS_GENERATOR, 1), 0);
        b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0);
        let to_array = b.emit(Op::JumpIfFalse(0), 0);
        self.compile_foreach_generator(b, &subj_t, key_var, val_var, pattern, body)?;
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

        self.loops.push(LoopCtx {
            breaks: vec![],
            continues: vec![],
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
    fn compile_foreach_generator(
        &mut self,
        b: &mut ChunkBuilder,
        subj_t: &str,
        key_var: Option<&str>,
        val_var: &str,
        pattern: Option<&Expr>,
        body: &[Stmt],
    ) -> Result<(), String> {
        // @subj->rewind();  (prime to the first yield)
        self.emit_get_var(b, subj_t);
        b.emit(Op::CallBuiltin(ops::GEN_REWIND, 1), 0);
        b.emit(Op::Pop, 0);

        let top = b.current_pos();
        // while (@subj->valid())
        self.emit_get_var(b, subj_t);
        b.emit(Op::CallBuiltin(ops::GEN_VALID, 1), 0);
        b.emit(Op::CallBuiltin(ops::TRUTHY, 1), 0);
        let exit = b.emit(Op::JumpIfFalse(0), 0);

        // bind key var (if any) and the value var from the current yield.
        if let Some(kv) = key_var {
            self.emit_set_var(b, kv, |c, b| {
                c.emit_get_var(b, subj_t);
                b.emit(Op::CallBuiltin(ops::GEN_KEY, 1), 0);
                Ok(())
            })?;
        }
        self.emit_set_var(b, val_var, |c, b| {
            c.emit_get_var(b, subj_t);
            b.emit(Op::CallBuiltin(ops::GEN_CURRENT, 1), 0);
            Ok(())
        })?;
        // A yielded value is a temporary with no element behind it, so a `&`
        // target in the pattern has nothing to alias.
        self.emit_foreach_destructure(b, pattern, val_var, None)?;

        self.loops.push(LoopCtx {
            breaks: vec![],
            continues: vec![],
        });
        self.compile_seq(b, body)?;
        let ctx = self.loops.pop().unwrap();

        // `continue` lands here → advance to the next yield.
        let cont_target = b.current_pos();
        self.emit_get_var(b, subj_t);
        b.emit(Op::CallBuiltin(ops::GEN_NEXT, 1), 0);
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
        if let Expr::Array(elems) = p {
            self.compile_list_targets(b, elems, val_var, ref_path)?;
            return Ok(());
        }
        self.compile_assign(b, p, None, &Expr::Var(val_var.to_string()))?;
        b.emit(Op::Pop, 0);
        Ok(())
    }

    /// Lower a class declaration to a `ClassDef`: constant and property-default
    /// initializers become standalone expression chunks (each leaving its value
    /// on the stack), and each method body compiles like a free function. A
    /// constructor with promoted parameters (`public int $x`) gets a synthetic
    /// `$this->x = $x;` prepended for each promoted parameter.
    fn compile_class(&mut self, b: &mut ChunkBuilder, decl: &ClassDecl) -> Result<(), String> {
        let prev_class = self.current_class.take();
        let prev_parent = self.current_parent.take();
        let prev_in_trait = std::mem::replace(&mut self.in_trait, decl.is_trait);
        self.current_class = Some(decl.name.clone());
        self.current_parent = decl.parent.clone();

        // Seed members from any used traits (declared earlier); the class's own
        // members below override them, matching PHP trait precedence.
        let mut consts: Vec<(String, Chunk)> = Vec::new();
        let mut prop_defaults: Vec<(String, Chunk)> = Vec::new();
        let mut static_prop_defaults: Vec<(String, Chunk)> = Vec::new();
        let mut methods: FxHashMap<String, FuncDef> = FxHashMap::default();
        let mut prop_vis: FxHashMap<String, Visibility> = FxHashMap::default();
        let mut readonly_props: FxHashSet<String> = FxHashSet::default();
        let mut method_vis: FxHashMap<String, Visibility> = FxHashMap::default();
        let mut order: Vec<String> = Vec::new();
        match self.seed_from_traits(
            decl,
            &mut consts,
            &mut prop_defaults,
            &mut static_prop_defaults,
            &mut methods,
            &mut prop_vis,
            &mut readonly_props,
            &mut method_vis,
            &mut order,
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
        }

        for prop in &decl.props {
            let name = &prop.name;
            prop_vis.insert(name.clone(), prop.visibility);
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

        for m in &decl.methods {
            method_vis.insert(m.name.to_ascii_lowercase(), m.visibility);
            order.retain(|n| !n.eq_ignore_ascii_case(&m.name));
            order.push(m.name.clone());
            let cparams = self.compile_params(&m.params)?;
            let mut mb = ChunkBuilder::new();
            // A method body has its own loop scope (as free functions do).
            let saved = std::mem::take(&mut self.loops);
            // Constructor property promotion: `public int $x` also assigns
            // `$this->x = $x` before the body runs.
            let mut promotions: Vec<Expr> = Vec::new();
            if m.name.eq_ignore_ascii_case("__construct") {
                for p in m.params.iter().filter(|p| p.promoted) {
                    // A promoted parameter DECLARES the property, so the synthetic
                    // assignment below must not read as creating a dynamic one.
                    // The parser does not keep which visibility keyword promoted
                    // it, and none is enforced on properties reached this way, so
                    // the declaration is recorded as public.
                    prop_vis.insert(p.name.clone(), Visibility::Public);
                    if p.readonly || decl.is_readonly {
                        readonly_props.insert(p.name.clone());
                    }
                    promotions.push(Expr::Assign(
                        Box::new(Expr::PropGet(
                            Box::new(Expr::Var("this".to_string())),
                            p.name.clone(),
                        )),
                        None,
                        Box::new(Expr::Var(p.name.clone())),
                    ));
                }
            }
            let saved_ref = std::mem::replace(&mut self.ret_by_ref, m.by_ref_return);
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
            self.decl_site = saved_site;
            self.ret_by_ref = saved_ref;
            self.loops = saved;
            methods.insert(
                m.name.to_ascii_lowercase(),
                FuncDef {
                    params: cparams,
                    // A method's frame is named by the method.
                    closure_site: None,
                    chunk: mb.build(),
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

        self.method_order
            .insert(decl.name.to_ascii_lowercase(), order);
        self.classes.push((
            decl.name.to_ascii_lowercase(),
            ClassDef {
                name: decl.name.clone(),
                parent: decl.parent.clone(),
                interfaces: decl.implements.clone(),
                consts,
                prop_defaults,
                static_prop_defaults,
                methods,
                prop_vis,
                readonly_props,
                method_vis,
                is_enum: decl.is_enum,
                is_abstract: decl.is_abstract,
                is_interface: decl.is_interface,
                allow_dynamic_props: decl
                    .attributes
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case("AllowDynamicProperties")),
                enum_cases,
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
        prop_defaults: &mut Vec<(String, Chunk)>,
        static_prop_defaults: &mut Vec<(String, Chunk)>,
        methods: &mut FxHashMap<String, FuncDef>,
        prop_vis: &mut FxHashMap<String, Visibility>,
        readonly_props: &mut FxHashSet<String>,
        method_vis: &mut FxHashMap<String, Visibility>,
        order: &mut Vec<String>,
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
            prop_defaults.extend(tdef.prop_defaults.iter().cloned());
            static_prop_defaults.extend(tdef.static_prop_defaults.iter().cloned());
            for (n, v) in &tdef.prop_vis {
                prop_vis.insert(n.clone(), *v);
            }
            // A property a trait declares readonly stays readonly in the class
            // that uses it — the trait is where it was declared.
            readonly_props.extend(tdef.readonly_props.iter().cloned());
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
            for spelling in self.declared_methods(&tdef.name) {
                let key = spelling.to_ascii_lowercase();
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
            order.push(self.method_spelling(&winner.name, m));
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
                    method_vis.insert(ak, vis);
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
        self.classes.iter().find(|(n, _)| *n == key).map(|(_, d)| d)
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
        self.in_try += 1;
        let r = self.compile_seq(&mut fb, body);
        self.in_try -= 1;
        self.loops = saved;
        r?;
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
        let id = self.try_defs.len() as i64;
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
            Expr::Array(elems) => {
                // `&` in a VALUE array (`$arr = [&$a]`, which makes the element
                // and `$a` one slot) is a different feature from the `&` target
                // this compiler supports, and needs an array element that can
                // hold a reference cell. Rejected rather than compiled as a
                // plain copy, which would answer silently and wrongly.
                if let Some(e) = elems.iter().find(|e| e.by_ref) {
                    let _ = e;
                    return Err("`&` in an array literal is supported only in a \
                                destructuring target, not in a value array"
                        .into());
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
            Expr::Assign(lhs, op, rhs) => self.compile_assign(b, lhs, *op, rhs)?,
            Expr::IncDec {
                target,
                inc,
                prefix,
            } => self.compile_incdec(b, target, *inc, *prefix)?,
            Expr::Call(name, args) if has_named(args) => {
                // A call with any `name: value` argument. Push the function name
                // then a `(name, value)` pair per argument for the host to rebind.
                let idx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(idx), 0);
                self.compile_arg_pairs_for(b, name, args)?;
                b.emit(
                    Op::CallBuiltin(ops::CALL_NAMED, (args.len() * 2 + 1) as u8),
                    self.cur_line,
                );
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
                    b.emit(Op::CallBuiltin(ops::ARR_MUT, (args.len() + 1) as u8), 0);
                } else if has_spread {
                    // Any `...$arr` argument switches to the spread dispatch: each
                    // argument is pushed as a `(is_spread, value)` pair so the host
                    // can flatten spread arrays into the positional argument list.
                    let idx = b.add_constant(Value::str(name.clone()));
                    b.emit(Op::LoadConst(idx), 0);
                    for a in args {
                        match a {
                            Expr::Spread(inner) => {
                                b.emit(Op::LoadTrue, 0);
                                self.compile_expr(b, inner)?;
                            }
                            _ => {
                                b.emit(Op::LoadFalse, 0);
                                self.compile_expr(b, a)?;
                            }
                        }
                    }
                    b.emit(
                        Op::CallBuiltin(ops::CALL_SPREAD, (args.len() * 2 + 1) as u8),
                        self.cur_line,
                    );
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
                    let byref = self.byref_positions(name, args.len());
                    let diag = byref_diag_slots(name, args.len());
                    for (i, a) in args.iter().enumerate() {
                        // An argument in a by-reference position is an output
                        // location, not a value the call reads, so an unset one is
                        // not a mistake and PHP raises no diagnostic for it —
                        // `preg_match($re, $s, $m)` with a fresh `$m` is the norm.
                        match &byref {
                            Some((p, _)) if p.contains(&i) => self.compile_quiet(b, a)?,
                            _ => self.compile_expr(b, a)?,
                        }
                        // Whether that position can actually be WRITTEN back to is
                        // a separate question from whether reading it is quiet,
                        // and it is settled here, between this argument and the
                        // next — the reference never evaluates the arguments after
                        // one it is about to reject.
                        if let Some(&(_, argno, param)) = diag.iter().find(|(p, ..)| *p == i) {
                            self.emit_byref_arg_diag(b, name, argno, param, byref_arg_class(a));
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
            Expr::CallValue(callee, args) if needs_arg_pairs(args) => {
                self.compile_expr(b, callee)?;
                self.compile_arg_pairs(b, args)?;
                b.emit(
                    Op::CallBuiltin(ops::CALLVALUE_NAMED, (args.len() * 2 + 1) as u8),
                    self.cur_line,
                );
            }
            Expr::CallValue(callee, args) => {
                self.compile_expr(b, callee)?;
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
                self.compile_closure(b, params, uses, body, ret.as_ref(), *is_static, *line)?;
            }
            Expr::ArrowFn {
                params,
                body,
                ret: ret_ty,
                line,
            } => {
                // An arrow fn desugars to a closure whose single-statement body
                // returns the expression; it captures every free variable of the
                // body (minus its own parameters) by value.
                let ret = vec![Stmt {
                    line: 0,
                    kind: StmtKind::Return(Some((**body).clone())),
                }];
                let mut captures = Vec::new();
                collect_free_vars(body, &mut captures);
                captures.retain(|n| !params.iter().any(|p| p.name == *n));
                // An arrow function has no `use` clause, so every capture is by
                // value — PHP has no by-reference form of it.
                let captures: Vec<Capture> = captures
                    .into_iter()
                    .map(|name| Capture {
                        name,
                        by_ref: false,
                    })
                    .collect();
                self.compile_closure(b, params, &captures, &ret, ret_ty.as_ref(), false, *line)?;
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
                self.compile_class(b, &named)?;
                self.cur_line = *line;
                self.compile_expr(b, &Expr::New(name, args.clone()))?;
                self.cur_line = site;
            }
            Expr::New(class, args) if needs_arg_pairs(args) => {
                self.emit_class_name(b, class)?;
                self.compile_arg_pairs(b, args)?;
                b.emit(
                    Op::CallBuiltin(ops::NEW_NAMED, (args.len() * 2 + 1) as u8),
                    self.cur_line,
                );
            }
            Expr::New(class, args) => {
                self.emit_class_name(b, class)?;
                for a in args {
                    self.compile_expr(b, a)?;
                }
                b.emit(
                    Op::CallBuiltin(ops::NEW, (args.len() + 1) as u8),
                    self.cur_line,
                );
            }
            Expr::PropGet(recv, name) => {
                self.compile_expr(b, recv)?;
                let idx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(idx), 0);
                b.emit(Op::CallBuiltin(ops::PROP_GET, 2), self.cur_line);
            }
            Expr::MethodCall(recv, name, args) if needs_arg_pairs(args) => {
                self.compile_expr(b, recv)?;
                let idx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(idx), 0);
                self.compile_arg_pairs(b, args)?;
                b.emit(
                    Op::CallBuiltin(ops::MCALL_NAMED, (args.len() * 2 + 2) as u8),
                    self.cur_line,
                );
            }
            Expr::MethodCall(recv, name, args) => {
                self.compile_expr(b, recv)?;
                let idx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(idx), 0);
                for a in args {
                    self.compile_expr(b, a)?;
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
                        ClassRef::Name(_) => self.emit_class_ref(b, class)?,
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
                b.emit(Op::LoadConst(nidx), 0);
                self.compile_arg_pairs(b, args)?;
                b.emit(
                    Op::CallBuiltin(ops::SCALL_NAMED, (args.len() * 2 + 2) as u8),
                    self.cur_line,
                );
            }
            Expr::StaticCall(class, name, args) => {
                self.emit_lsb_forward(b, class);
                self.emit_class_ref(b, class)?;
                let nidx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(nidx), 0);
                for a in args {
                    self.compile_expr(b, a)?;
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
                    let idx = b.add_constant(Value::str(name.clone()));
                    b.emit(Op::LoadConst(idx), self.cur_line);
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
                    let idx = b.add_constant(Value::str(name.clone()));
                    b.emit(Op::LoadConst(idx), self.cur_line);
                    b.emit(Op::CallBuiltin(ops::PROP_GET_EMPTY, 2), self.cur_line);
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
                        b.emit(Op::CallBuiltin(ops::YIELD_KV, 2), 0);
                    }
                    None => {
                        b.emit(Op::CallBuiltin(ops::YIELD, 1), 0);
                    }
                }
            }
            Expr::YieldFrom(src) => {
                self.compile_expr(b, src)?;
                b.emit(Op::CallBuiltin(ops::YIELD_FROM, 1), 0);
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
                let idx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(idx), line);
                b.emit(Op::CallBuiltin(ops::PROP_GET_Q, 2), line);
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
                let pi = b.add_constant(Value::str(prop.clone()));
                b.emit(Op::LoadConst(pi), 0);
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
                let pi = b.add_constant(Value::str(prop.clone()));
                b.emit(Op::LoadConst(pi), 0);
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
                    let pi = b.add_constant(Value::str(prop));
                    b.emit(Op::LoadConst(pi), 0);
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
        match t {
            Expr::PropGet(recv, prop) => {
                self.compile_expr(b, recv)?;
                let pi = b.add_constant(Value::str(prop.clone()));
                b.emit(Op::LoadConst(pi), self.cur_line);
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
        // Only `*`. `+` is not swapped even though it commutes on numbers —
        // it is also array union, which does not. `-`, `/`, `%` and `**` all
        // report in source order.
        let swap = op == BinOp::Mul && is_const_operand(l) && is_definitely_runtime(r);
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

    fn compile_assign(
        &mut self,
        b: &mut ChunkBuilder,
        lhs: &Expr,
        op: Option<BinOp>,
        rhs: &Expr,
    ) -> Result<(), String> {
        match lhs {
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
                        self.compile_expr(b, recv)?;
                        let nidx = b.add_constant(Value::str(name.clone()));
                        b.emit(Op::LoadConst(nidx), 0);
                        self.compile_rhs(b, rhs)?;
                        b.emit(Op::CallBuiltin(ops::PROP_SET, 3), self.cur_line);
                    }
                    Some(cop) => {
                        let r = self.tmp_name("pr");
                        self.emit_set_var(b, &r, |c, b| c.compile_expr(b, recv))?;
                        // Fetch-for-write comes FIRST, so `$o->missing .= "x"`
                        // deprecates the dynamic property before the read below
                        // warns that it is undefined — the reference order.
                        self.emit_get_var(b, &r);
                        let tidx = b.add_constant(Value::str(name.clone()));
                        b.emit(Op::LoadConst(tidx), 0);
                        b.emit(Op::CallBuiltin(ops::PROP_TOUCH, 2), self.cur_line);
                        let nidx = b.add_constant(Value::str(name.clone()));
                        b.emit(Op::LoadConst(nidx), 0);
                        // value = @r->name op rhs
                        self.emit_get_var(b, &r);
                        let gidx = b.add_constant(Value::str(name.clone()));
                        b.emit(Op::LoadConst(gidx), 0);
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
                            c.compile_expr(b, recv)?;
                            let idx = b.add_constant(Value::str(prop.clone()));
                            b.emit(Op::LoadConst(idx), 0);
                            b.emit(Op::CallBuiltin(ops::PROP_ENSURE_ARRAY, 2), c.cur_line);
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
                b.emit(Op::CallBuiltin(ops::SPROP_SET, 3), 0);
            }
            // List destructuring — `list($a,$b) = …`, `[$a,$b] = …`, and the keyed
            // form `['k' => $v] = …`. Both `list(...)` and `[...]` parse to
            // `Expr::Array`, so this one arm serves every syntax. The RHS is
            // evaluated once into a temp (its value is also the assignment
            // expression's result, matching PHP), then each element target is
            // assigned `@src[key]`. Unkeyed elements take successive integer
            // indices; a `Null` element is a hole (`[,$b]`) that still consumes an
            // index but binds nothing; a nested `Expr::Array` target recurses.
            Expr::Array(elems) => {
                if op.is_some() {
                    return Err("compound assignment cannot target a list()/[] pattern".into());
                }
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
                    Expr::Array(inner) => Self::pattern_binds_by_ref(inner),
                    _ => false,
                }
        })
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
            if matches!(e.value, Expr::Null) {
                continue;
            }
            let elem = Expr::ListElem(
                Box::new(Expr::Var(value_tmp.to_string())),
                Box::new(key.clone()),
            );
            let deeper = ref_path.map(|p| Expr::Index(Box::new(p.clone()), Box::new(key.clone())));
            match &e.value {
                // A nested pattern recurses, carrying both sources down.
                Expr::Array(inner) => {
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
        let cparams = self.compile_params(params)?;
        let mut fb = ChunkBuilder::new();
        // Like a named function, the body gets its own loop scope so a `break`
        // inside it cannot target a loop at the creation site.
        let saved = std::mem::take(&mut self.loops);
        // This literal's own site, which names its frames — and which a closure
        // written INSIDE it nests under, so `{closure:{closure:f.php:2}:3}`
        // falls out of the same rule rather than being a second case.
        let site = host::DeclSite::Closure(Box::new(self.decl_site.clone()), line);
        let saved_site = std::mem::replace(&mut self.decl_site, site.clone());
        self.in_other_frame(|c| c.compile_seq(&mut fb, body))?;
        self.decl_site = saved_site;
        self.loops = saved;
        let def_name = self.tmp_name("closure");
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
                self.compile_expr(b, recv)?;
                let nidx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(nidx), 0);
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
                            let idx = b.add_constant(Value::str(prop.clone()));
                            b.emit(Op::LoadConst(idx), 0);
                            b.emit(Op::CallBuiltin(ops::PROP_ENSURE_ARRAY, 2), c.cur_line);
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
        if quiet {
            self.compile_quiet(b, base)?;
        } else {
            self.compile_expr(b, base)?;
        }
        // Pending jumps to the chain's end — one per `?->` that short-circuits.
        let mut exits = Vec::new();
        for link in spine.iter().rev() {
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
            self.compile_chain_link(b, link, quiet)?;
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
                let idx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(idx), line);
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
                let idx = b.add_constant(Value::str(name.clone()));
                b.emit(Op::LoadConst(idx), line);
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
        self.compile_arg_pairs_for(b, "", args)
    }

    /// [`Compiler::compile_arg_pairs`], plus the by-reference argument check for
    /// a call whose callee is known by name.
    ///
    /// A named argument reaches a by-reference parameter just as a positional
    /// one does, so `sort(array: [1, 2])` is the same error as `sort([1, 2])`.
    /// Which slot it lands in is found by NAME rather than by position, which is
    /// the whole point of the syntax.
    fn compile_arg_pairs_for(
        &mut self,
        b: &mut ChunkBuilder,
        callee: &str,
        args: &[Expr],
    ) -> Result<(), String> {
        let diag = byref_diag_slots(callee, args.len().max(BYREF_MAX_ARGNO));
        for (i, a) in args.iter().enumerate() {
            let slot = match a {
                Expr::NamedArg(n, v) => {
                    let idx = b.add_constant(Value::str(n.clone()));
                    b.emit(Op::LoadConst(idx), 0);
                    self.compile_expr(b, v)?;
                    diag.iter().find(|(_, _, param)| param == n).copied()
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
                    self.compile_expr(b, a)?;
                    diag.iter().find(|(p, ..)| *p == i).copied()
                }
            };
            if let Some((_, argno, param)) = slot {
                let inner = match a {
                    Expr::NamedArg(_, v) => v,
                    _ => a,
                };
                self.emit_byref_arg_diag(b, callee, argno, param, byref_arg_class(inner));
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
        Expr::Array(elems) => {
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
        Expr::StaticCall(class, _, args) => {
            if let ClassRef::Expr(c) = class {
                collect_free_vars(c, out);
            }
            for a in args {
                collect_free_vars(a, out);
            }
        }
        Expr::PropGet(recv, _) | Expr::NullsafePropGet(recv, _) => collect_free_vars(recv, out),
        Expr::MethodCall(recv, _, args) | Expr::NullsafeMethodCall(recv, _, args) => {
            collect_free_vars(recv, out);
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
        Expr::Null | Expr::Bool(_) | Expr::Int(_) | Expr::Float(_) | Expr::Str(_) => {}
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
        | Expr::PropGet(a, _)
        | Expr::NullsafePropGet(a, _)
        | Expr::Throw(a)
        | Expr::Clone(a)
        | Expr::InstanceOf(a, _)
        | Expr::NamedArg(_, a) => expr_has_yield(a),
        Expr::Binary(_, a, b)
        | Expr::Elvis(a, b)
        | Expr::Coalesce(a, b)
        | Expr::RefAssign(a, b) => expr_has_yield(a) || expr_has_yield(b),
        Expr::Assign(a, _, b) => expr_has_yield(a) || expr_has_yield(b),
        Expr::Ternary(a, c, d) => expr_has_yield(a) || expr_has_yield(c) || expr_has_yield(d),
        Expr::IncDec { target, .. } => expr_has_yield(target),
        Expr::Call(_, args) | Expr::New(_, args) | Expr::NewAnon { args, .. } => {
            args.iter().any(expr_has_yield)
        }
        Expr::StaticCall(class, _, args) => {
            class.operand().is_some_and(expr_has_yield) || args.iter().any(expr_has_yield)
        }
        Expr::StaticGet(class, _) | Expr::StaticProp(class, _) => {
            class.operand().is_some_and(expr_has_yield)
        }
        Expr::CallValue(c, args) => expr_has_yield(c) || args.iter().any(expr_has_yield),
        Expr::MethodCall(r, _, args) | Expr::NullsafeMethodCall(r, _, args) => {
            expr_has_yield(r) || args.iter().any(expr_has_yield)
        }
        Expr::Array(items) => items
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
    if depth == 0 {
        format!("'{kw}' outside of a loop")
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
            | Expr::Array(_)
    )
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
        Expr::IncDec { .. } | Expr::InstanceOf(..) | Expr::IssetOf(_) | Expr::EmptyOf(_) => true,
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
            Expr::Array(elems) => {
                for el in elems {
                    if let Some(k) = &el.key {
                        self.expr(k);
                    }
                    self.expr(&el.value);
                }
            }
            Expr::Index(a, b) | Expr::ListElem(a, b) | Expr::Binary(_, a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Append(a)
            | Expr::Unary(_, a)
            | Expr::Spread(a)
            | Expr::Clone(a)
            | Expr::Throw(a)
            | Expr::Quiet(a)
            | Expr::NamedArg(_, a)
            | Expr::PropGet(a, _)
            | Expr::NullsafePropGet(a, _)
            | Expr::InstanceOf(a, _) => self.expr(a),
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
            Expr::MethodCall(r, _, args) | Expr::NullsafeMethodCall(r, _, args) => {
                self.expr(r);
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
