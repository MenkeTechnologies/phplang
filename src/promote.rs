//! Which locals may live in a fusevm FRAME SLOT instead of the host scope.
//!
//! phplang keeps every PHP variable in `PhpHost::scopes`, reached through
//! `CallBuiltin(GETSLOT)` / `CallBuiltin(SETSLOT)`. That is what
//! [`PhpHost::slot_get`](crate::host::PhpHost::slot_get) needs in order to do
//! three things a bare frame slot cannot:
//!
//! 1. tell `Slot::Unset` from a bound null, so a read can warn `Undefined
//!    variable $x`;
//! 2. read THROUGH a `Slot::Ref` cell, so `&$x`, `global` and a by-reference
//!    parameter all see one value;
//! 3. answer a lookup by NAME, which `$$x`, `extract`, `compact` and
//!    `get_defined_vars` all make.
//!
//! A variable that provably needs none of the three can be held in the frame
//! instead, as `Op::GetSlot` / `Op::SetSlot`. That is worth doing because
//! `CallBuiltin` is the one op fusevm's tracing JIT refuses outright
//! (`jit.rs`, `is_trace_op_allowed_at`), so a loop whose body touches a
//! variable can never be compiled — the refusal happens before branch shape is
//! even looked at.
//!
//! This module decides only WHICH names qualify. It never has to be complete:
//! a name it declines keeps today's by-name path, which is correct and merely
//! slower. Every rule below is therefore written to reject on doubt, and the
//! scope is abandoned entirely for constructs that can reach a local without
//! naming it in the source.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::ast::{Expr, ForeachVal, Param, Stmt, StmtKind};

/// Names of the locals in one scope that may be held in a frame slot, in the
/// order they should be numbered.
///
/// `by_ref_arg(name, index)` answers whether argument `index` of a call to
/// `name` is taken by reference — the caller supplies it because the compiler
/// already keeps that table for both builtins and user functions.
/// The promoted locals of a scope, and which of them provably only ever hold a
/// number.
pub struct Promoted {
    /// Names to hold in frame slots, in the order to number them.
    pub names: Vec<String>,
    /// Of those, the ones every write to is a numeric literal or a step of
    /// themselves. For such a name `++` is exactly `+ 1`, which matters because
    /// `+` is a native op and PHP's `++` is not: `"Az"++` is `"Ba"` and `true++`
    /// is a no-op, so the general form has to ask the host.
    pub numeric: FxHashSet<String>,
}

pub fn promotable(
    params: &[Param],
    body: &[Stmt],
    is_generator: bool,
    by_ref_arg: &dyn Fn(&str, usize) -> bool,
) -> Promoted {
    // A generator body is suspended and resumed across `yield`, and its frame
    // does not survive that the way the host scope does.
    if is_generator {
        return Promoted {
            names: Vec::new(),
            numeric: FxHashSet::default(),
        };
    }
    let mut s = Scan {
        banned: FxHashSet::default(),
        poisoned: false,
        detached: false,
        by_ref_arg,
    };
    // A parameter is bound BY NAME when the frame is built, so it is never a
    // candidate however it is used afterwards.
    for p in params {
        s.ban(&p.name);
    }
    s.stmts(body);
    if s.poisoned {
        return Promoted {
            names: Vec::new(),
            numeric: FxHashSet::default(),
        };
    }
    let banned = s.banned;

    // Second pass: a frame slot has no "unset" state, so a read that could
    // happen before the first write would answer null where the host scope
    // answers null AND warns. Only names written before every read survive.
    let mut flow = Flow {
        assigned: FxHashSet::default(),
        banned,
        order: Vec::new(),
        seen: FxHashSet::default(),
        non_numeric: FxHashSet::default(),
    };
    flow.stmts(body);
    let names: Vec<String> = flow
        .order
        .into_iter()
        .filter(|n| !flow.banned.contains(n))
        .collect();
    let numeric = names
        .iter()
        .filter(|n| !flow.non_numeric.contains(*n))
        .cloned()
        .collect();
    Promoted { names, numeric }
}

/// Whether `e` is a literal number — the only right-hand side that keeps a
/// variable provably numeric without knowing anything else about it.
fn numeric_literal(e: &Expr) -> bool {
    matches!(e, Expr::Int(_) | Expr::Float(_))
}

/// Whether a destructuring pattern binds any of its targets by reference.
fn pattern_binds_by_ref(e: &Expr) -> bool {
    match e {
        Expr::Array(elems) => elems
            .iter()
            .any(|el| el.by_ref || pattern_binds_by_ref(&el.value)),
        Expr::ListElem(_, v) => pattern_binds_by_ref(v),
        _ => false,
    }
}

/// Whether a name is eligible at all — the same exclusions the host slot
/// numbering makes, since a superglobal and `$this` are not frame storage.
fn eligible(name: &str) -> bool {
    !crate::host::is_superglobal(name) && name != "this" && !name.starts_with('@')
}

// ── pass 1: what disqualifies a name, or the whole scope ─────────────────────

struct Scan<'a> {
    banned: FxHashSet<String>,
    poisoned: bool,
    /// While set, every variable MENTIONED is banned rather than merely walked.
    ///
    /// A `try`/`catch`/`finally` body is compiled as its own detached chunk and
    /// run on its own VM, so it has a different frame. The host scope is shared
    /// between the two and a frame slot is not, so any name the body touches has
    /// to stay in the host storage both chunks can see.
    detached: bool,
    by_ref_arg: &'a dyn Fn(&str, usize) -> bool,
}

impl Scan<'_> {
    fn ban(&mut self, n: &str) {
        self.banned.insert(n.to_string());
    }

    /// The whole scope is abandoned: something in it can reach a local without
    /// naming it, so no per-name rule can be trusted.
    fn poison(&mut self) {
        self.poisoned = true;
    }

    /// The root variable of an lvalue PATH (`$a[0]`, `$a[]`, `$a->p`).
    ///
    /// Writing through a path reaches the host scope by name or host slot, so
    /// the root cannot also be living in a frame slot.
    fn ban_path_root(&mut self, e: &Expr) {
        match e {
            Expr::Var(n) => self.ban(n),
            Expr::Index(r, _) | Expr::Append(r) | Expr::PropGet(r, _) => self.ban_path_root(r),
            // A destructuring pattern binds every variable inside it, and `[&$a]`
            // binds one of them by reference.
            Expr::Array(_) | Expr::ListElem(..) => self.ban_all_vars(e),
            _ => {}
        }
    }

    /// Ban every variable anywhere in `e`.
    ///
    /// Used for an assignment target that is not a plain `$x`: the write goes
    /// through machinery that reaches the variable by name, and for a
    /// destructuring pattern it may bind by reference as well.
    fn ban_all_vars(&mut self, e: &Expr) {
        let saved = self.detached;
        self.detached = true;
        self.expr(e);
        self.detached = saved;
    }

    /// Walk `body` with every name it mentions banned.
    fn detached_stmts(&mut self, body: &[Stmt]) {
        let saved = self.detached;
        self.detached = true;
        self.stmts(body);
        self.detached = saved;
    }

    fn stmts(&mut self, body: &[Stmt]) {
        for s in body {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Echo(es) => self.exprs(es),
            StmtKind::Expr(e) | StmtKind::Return(Some(e)) => self.expr(e),
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
            // A `foreach` binds its key and value through the host scope (and
            // `as &$v` binds a reference), so neither is a candidate.
            StmtKind::Foreach {
                arr,
                key_var,
                val,
                body,
                ..
            } => {
                self.expr(arr);
                if let Some(k) = key_var {
                    self.ban(k);
                }
                match val {
                    ForeachVal::Var(n) => self.ban(n),
                    ForeachVal::Pattern(e) => self.ban_all_vars(e),
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
                self.detached_stmts(body);
                for c in catches {
                    // The caught variable is bound by name when the handler runs.
                    if let Some(v) = &c.var {
                        self.ban(v);
                    }
                    self.detached_stmts(&c.body);
                }
                if let Some(f) = finally {
                    self.detached_stmts(f);
                }
            }
            StmtKind::Block(b) => self.stmts(b),
            // `global $x` and `static $x` both bind the name to storage outside
            // this frame — a shared reference cell and a per-declaration slot.
            StmtKind::Global(names) => {
                for n in names {
                    self.ban(n);
                }
            }
            StmtKind::StaticLocal(decls) => {
                for (n, init) in decls {
                    self.ban(n);
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
            // A nested declaration compiles against its own frame.
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

    /// Ban every variable a call passes in a by-reference position, and every
    /// variable passed to a call whose callee is not known at compile time.
    fn call_args(&mut self, callee: Option<&str>, args: &[Expr]) {
        for (i, a) in args.iter().enumerate() {
            let inner = match a {
                Expr::NamedArg(_, v) => v,
                Expr::Spread(v) => v,
                other => other,
            };
            match callee {
                // A named callee has a known by-reference signature.
                Some(name) if !(self.by_ref_arg)(name, i) => {}
                // Either a by-reference position, or a callee this call site
                // cannot name — a dynamic call could be `extract` itself.
                _ => self.ban_path_root(inner),
            }
            self.expr(inner);
        }
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            // A dynamic name can reach ANY local, so nothing in this scope can
            // move out of the host storage it looks names up in.
            Expr::VarVar(_) => self.poison(),
            Expr::Var(n) => {
                if self.detached {
                    self.ban(n);
                }
            }
            Expr::Null | Expr::Bool(_) | Expr::Int(_) | Expr::Float(_) => {}
            Expr::Str(_) | Expr::ConstFetch(_) | Expr::Magic(_) => {}
            // `$cls::K` / `$cls::$p` / `$cls::m()` — the LEFT of a `::` can be an
            // expression, and it reads a variable. Skipping it (which every one
            // of these arms used to do) hid that read from this analysis, so a
            // variable reached ONLY through `$v::` was promoted into a frame slot
            // and then read as null by any detached chunk — a `try` body, most
            // visibly: `try { echo $o::class; }` answered
            // `Cannot use "::class" on null`.
            Expr::StaticGet(class, _) => {
                if let Some(op) = class.operand() {
                    self.expr(op);
                }
            }
            Expr::Interp(parts) => {
                for p in parts {
                    if let crate::ast::InterpPart::Expr(e) = p {
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
            Expr::Index(r, k) => {
                self.expr(r);
                self.expr(k);
            }
            Expr::ListElem(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Append(r) => self.expr(r),
            Expr::Unary(_, x) | Expr::Spread(x) | Expr::NamedArg(_, x) => self.expr(x),
            Expr::Binary(_, a, b) | Expr::Elvis(a, b) | Expr::Coalesce(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Assign(lhs, _, rhs) => {
                // Writing through a PATH reaches the root by name, so the root
                // stays in the host scope. A plain `$x = …` is the shape that
                // can move, and is left to the definite-assignment pass.
                if !matches!(lhs.as_ref(), Expr::Var(_)) {
                    // Anything other than a plain `$x` writes through a path or
                    // a pattern, both of which name the variable at run time.
                    self.ban_all_vars(lhs);
                    // `[&$x] = $a` makes `$x` an alias of an ELEMENT of `$a`, so
                    // the subject has to be reachable by name as well — a
                    // promoted subject would be copied to a temporary and the
                    // alias would write to the copy.
                    if pattern_binds_by_ref(lhs) {
                        self.ban_all_vars(rhs);
                    }
                }
                self.expr(lhs);
                self.expr(rhs);
            }
            Expr::IncDec { target, .. } => {
                if !matches!(target.as_ref(), Expr::Var(_)) {
                    self.ban_path_root(target);
                }
                self.expr(target);
            }
            // `$a = &$b` ties two names to one cell; neither can be a frame slot.
            Expr::RefAssign(a, b) => {
                self.ban_path_root(a);
                self.ban_path_root(b);
                self.expr(a);
                self.expr(b);
            }
            Expr::Call(name, args) => {
                let bare = name
                    .rsplit('\\')
                    .next()
                    .unwrap_or(name)
                    .to_ascii_lowercase();
                // These read or write the caller's variables BY NAME. A local
                // held in a frame slot is invisible to them, so the scope keeps
                // its host storage rather than diverging.
                if matches!(
                    bare.as_str(),
                    "extract"
                        | "compact"
                        | "get_defined_vars"
                        | "eval"
                        | "parse_str"
                        | "settype"
                        | "func_get_args"
                ) {
                    self.poison();
                }
                self.call_args(Some(&bare), args);
            }
            // The callee is a value, so its by-reference signature — and even
            // its identity — is unknown here.
            Expr::CallValue(f, args) => {
                self.expr(f);
                self.call_args(None, args);
            }
            Expr::MethodCall(r, _, args) | Expr::NullsafeMethodCall(r, _, args) => {
                self.expr(r);
                self.call_args(None, args);
            }
            Expr::StaticCall(class, _, args) => {
                if let Some(op) = class.operand() {
                    self.expr(op);
                }
                self.call_args(None, args);
            }
            Expr::New(_, args) => self.call_args(None, args),
            Expr::NewAnon { args, .. } => self.call_args(None, args),
            // A closure captures by NAME at creation; an arrow function does the
            // same for every free variable of its body. Either way the captured
            // name has to be readable from the host scope.
            Expr::Closure { uses, .. } => {
                for u in uses {
                    self.ban(&u.name);
                }
            }
            Expr::ArrowFn { params, body, .. } => {
                let mut free = Vec::new();
                crate::compiler::collect_free_vars(body, &mut free);
                for n in free {
                    if !params.iter().any(|p| p.name == n) {
                        self.ban(&n);
                    }
                }
            }
            Expr::PropGet(r, _) | Expr::NullsafePropGet(r, _) => self.expr(r),
            Expr::StaticProp(class, _) => {
                if let Some(op) = class.operand() {
                    self.expr(op);
                }
            }
            Expr::Clone(x) | Expr::Throw(x) | Expr::YieldFrom(x) | Expr::Print(x) => self.expr(x),
            Expr::InstanceOf(x, _) => self.expr(x),
            Expr::Ternary(a, b, c) => {
                self.expr(a);
                self.expr(b);
                self.expr(c);
            }
            Expr::Match { subj, arms } => {
                self.expr(subj);
                for a in arms {
                    for c in a.conds.iter().flatten() {
                        self.expr(c);
                    }
                    self.expr(&a.body);
                }
            }
            // `unset($x)` returns the name to its unset state, which a frame
            // slot cannot represent.
            Expr::Unset(targets) => {
                for t in targets {
                    self.ban_path_root(t);
                }
            }
            Expr::IssetOf(x) | Expr::EmptyOf(x) | Expr::Quiet(x) | Expr::Suppress(x) => {
                self.expr(x)
            }
            Expr::Yield { key, value } => {
                if let Some(k) = key {
                    self.expr(k);
                }
                if let Some(v) = value {
                    self.expr(v);
                }
            }
        }
    }
}

// ── pass 2: definite assignment ──────────────────────────────────────────────

/// Tracks which names are certainly written by the time control reaches a
/// point. A read of anything else bans that name: a frame slot cannot tell
/// "never written" from "written null", and the reference distinguishes them.
struct Flow {
    assigned: FxHashSet<String>,
    banned: FxHashSet<String>,
    /// Candidates in first-assignment order, which is the order they are
    /// numbered in so a chunk's slot indices stay stable.
    order: Vec<String>,
    seen: FxHashSet<String>,
    /// Names with a write this pass could not prove keeps them numeric.
    non_numeric: FxHashSet<String>,
}

impl Flow {
    fn read(&mut self, n: &str) {
        if eligible(n) && !self.assigned.contains(n) {
            self.banned.insert(n.to_string());
        }
    }

    fn write(&mut self, n: &str) {
        if !eligible(n) {
            return;
        }
        self.assigned.insert(n.to_string());
        if self.seen.insert(n.to_string()) {
            self.order.push(n.to_string());
        }
    }

    /// Run `f` over a branch that MAY be skipped, keeping its reads but
    /// discarding the assignments it makes — they are not certain afterwards.
    fn maybe(&mut self, f: impl FnOnce(&mut Self)) {
        let saved = self.assigned.clone();
        f(self);
        self.assigned = saved;
    }

    fn stmts(&mut self, body: &[Stmt]) {
        for s in body {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Echo(es) => self.exprs(es),
            StmtKind::Expr(e) | StmtKind::Return(Some(e)) => self.expr(e),
            StmtKind::If {
                cond,
                then,
                elifs,
                els,
            } => {
                self.expr(cond);
                // Every arm is optional on its own, and there is no way to know
                // which ran, so none of their writes survive the statement.
                self.maybe(|f| f.stmts(then));
                for (c, b) in elifs {
                    self.expr(c);
                    self.maybe(|f| f.stmts(b));
                }
                if let Some(b) = els {
                    self.maybe(|f| f.stmts(b));
                }
            }
            // The body may run zero times.
            StmtKind::While { cond, body } => {
                self.expr(cond);
                self.maybe(|f| f.stmts(body));
            }
            // …but a do-while body always runs once.
            StmtKind::DoWhile { cond, body } => {
                self.stmts(body);
                self.expr(cond);
            }
            StmtKind::For {
                init,
                cond,
                step,
                body,
            } => {
                // The initializer always runs, and runs first; the condition is
                // then evaluated before the body ever is, so the body's writes
                // are not available to it.
                self.exprs(init);
                if let Some(c) = cond {
                    self.expr(c);
                }
                self.maybe(|f| {
                    f.stmts(body);
                    f.exprs(step);
                });
            }
            StmtKind::Foreach { arr, body, .. } => {
                self.expr(arr);
                self.maybe(|f| f.stmts(body));
            }
            StmtKind::Switch { subj, cases } => {
                self.expr(subj);
                for c in cases {
                    if let Some(e) = &c.test {
                        self.expr(e);
                    }
                    self.maybe(|f| f.stmts(&c.body));
                }
            }
            // A throw can leave the body at any point, so nothing it wrote is
            // certain once a handler is running or the statement is past.
            StmtKind::Try {
                body,
                catches,
                finally,
            } => {
                self.maybe(|f| f.stmts(body));
                for c in catches {
                    self.maybe(|f| f.stmts(&c.body));
                }
                if let Some(fin) = finally {
                    self.maybe(|f| f.stmts(fin));
                }
            }
            StmtKind::Block(b) => self.stmts(b),
            StmtKind::StaticLocal(decls) => {
                for (_, init) in decls {
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
            StmtKind::Global(_)
            | StmtKind::Function { .. }
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
            Expr::Var(n) => self.read(n),
            Expr::Assign(lhs, op, rhs) => {
                self.expr(rhs);
                match lhs.as_ref() {
                    Expr::Var(n) => {
                        // A compound assignment reads the old value first.
                        if op.is_some() {
                            self.read(n);
                        }
                        // A literal number keeps it numeric; arithmetic against
                        // one keeps a number a number. Anything else is unknown.
                        if !numeric_literal(rhs) {
                            self.non_numeric.insert(n.to_string());
                        }
                        self.write(n);
                    }
                    other => self.expr(other),
                }
            }
            Expr::IncDec { target, .. } => {
                if let Expr::Var(n) = target.as_ref() {
                    self.read(n);
                    self.write(n);
                } else {
                    self.expr(target);
                }
            }
            // A quiet read asks whether the name is bound, which is exactly the
            // question a frame slot cannot answer, so it counts as a read.
            Expr::IssetOf(x) | Expr::EmptyOf(x) | Expr::Quiet(x) | Expr::Suppress(x) => {
                self.expr(x)
            }
            Expr::Interp(parts) => {
                for p in parts {
                    if let crate::ast::InterpPart::Expr(e) = p {
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
            Expr::Index(a, b) | Expr::ListElem(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Append(x) | Expr::Unary(_, x) | Expr::Spread(x) | Expr::NamedArg(_, x) => {
                self.expr(x)
            }
            Expr::Binary(_, a, b) | Expr::Elvis(a, b) | Expr::Coalesce(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::RefAssign(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Call(_, args) | Expr::New(_, args) => self.exprs(args),
            Expr::StaticCall(class, _, args) => {
                if let Some(op) = class.operand() {
                    self.expr(op);
                }
                self.exprs(args);
            }
            Expr::NewAnon { args, .. } => self.exprs(args),
            Expr::CallValue(f, args) => {
                self.expr(f);
                self.exprs(args);
            }
            Expr::MethodCall(r, _, args) | Expr::NullsafeMethodCall(r, _, args) => {
                self.expr(r);
                self.exprs(args);
            }
            Expr::PropGet(r, _) | Expr::NullsafePropGet(r, _) => self.expr(r),
            Expr::Clone(x)
            | Expr::Throw(x)
            | Expr::YieldFrom(x)
            | Expr::Print(x)
            | Expr::InstanceOf(x, _) => self.expr(x),
            Expr::Ternary(a, b, c) => {
                self.expr(a);
                self.maybe(|f| f.expr(b));
                self.maybe(|f| f.expr(c));
            }
            Expr::Match { subj, arms } => {
                self.expr(subj);
                for a in arms {
                    for c in a.conds.iter().flatten() {
                        self.expr(c);
                    }
                    self.maybe(|f| f.expr(&a.body));
                }
            }
            Expr::Unset(ts) => self.exprs(ts),
            Expr::Yield { key, value } => {
                if let Some(k) = key {
                    self.expr(k);
                }
                if let Some(v) = value {
                    self.expr(v);
                }
            }
            Expr::VarVar(x) => self.expr(x),
            Expr::Closure { .. } | Expr::ArrowFn { .. } => {}
            Expr::Null
            | Expr::Bool(_)
            | Expr::Int(_)
            | Expr::Float(_)
            | Expr::Str(_)
            | Expr::ConstFetch(_)
            | Expr::Magic(_) => {}
            // `$cls::K` / `$cls::$p` / `$cls::m()` — the LEFT of a `::` can be an
            // expression, and it reads a variable. Skipping it (which every one
            // of these arms used to do) hid that read from this analysis, so a
            // variable reached ONLY through `$v::` was promoted into a frame slot
            // and then read as null by any detached chunk — a `try` body, most
            // visibly: `try { echo $o::class; }` answered
            // `Cannot use "::class" on null`.
            Expr::StaticGet(class, _) | Expr::StaticProp(class, _) => {
                if let Some(op) = class.operand() {
                    self.expr(op);
                }
            }
        }
    }
}

/// Which TOP-LEVEL names another scope can reach, and whether the question can
/// be answered at all.
///
/// A top-level local is a PHP global: `global $x` in any function binds it, and
/// `$GLOBALS['x']` reads it. Both go through the host scope by name, so a
/// top-level variable either of them can reach must stay there.
///
/// `None` means the program mentions `$GLOBALS`, whose subscript may be computed
/// — no name is safe then, and the top level promotes nothing.
///
/// This walks the WHOLE program, nested function, method and closure bodies
/// included, because the reference to a global is written inside them.
pub fn globals_reached(stmts: &[Stmt]) -> Option<FxHashSet<String>> {
    let mut g = GlobalScan {
        names: FxHashSet::default(),
        any_globals_array: false,
    };
    g.stmts(stmts);
    (!g.any_globals_array).then_some(g.names)
}

struct GlobalScan {
    names: FxHashSet<String>,
    any_globals_array: bool,
}

impl GlobalScan {
    fn stmts(&mut self, body: &[Stmt]) {
        for s in body {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Global(names) => {
                for n in names {
                    self.names.insert(n.clone());
                }
            }
            StmtKind::Echo(es) => self.exprs(es),
            StmtKind::Expr(e) | StmtKind::Return(Some(e)) => self.expr(e),
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
            StmtKind::Foreach { arr, body, .. } => {
                self.expr(arr);
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
                    self.stmts(&c.body);
                }
                if let Some(f) = finally {
                    self.stmts(f);
                }
            }
            StmtKind::Block(b) => self.stmts(b),
            // Unlike the per-scope scan, this one DOES descend into nested
            // declarations: that is where a `global` is written.
            StmtKind::Function { body, .. } => self.stmts(body),
            StmtKind::Class(decl) => {
                for m in &decl.methods {
                    self.stmts(&m.body);
                }
            }
            StmtKind::StaticLocal(decls) => {
                for (_, init) in decls {
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
            StmtKind::InlineHtml(_)
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
            Expr::Var(n) => {
                if n == "GLOBALS" {
                    self.any_globals_array = true;
                }
            }
            // A closure body is its own scope, but a `global` written inside it
            // still names a TOP-LEVEL variable, so it is walked from here.
            Expr::Closure { body, .. } => self.stmts(body),
            Expr::ArrowFn { body, .. } => self.expr(body),
            Expr::Interp(parts) => {
                for p in parts {
                    if let crate::ast::InterpPart::Expr(x) = p {
                        self.expr(x);
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
            Expr::Index(a, b)
            | Expr::ListElem(a, b)
            | Expr::Binary(_, a, b)
            | Expr::Elvis(a, b)
            | Expr::Coalesce(a, b)
            | Expr::RefAssign(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Assign(a, _, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Append(x)
            | Expr::Unary(_, x)
            | Expr::Spread(x)
            | Expr::NamedArg(_, x)
            | Expr::Clone(x)
            | Expr::Throw(x)
            | Expr::YieldFrom(x)
            | Expr::Print(x)
            | Expr::InstanceOf(x, _)
            | Expr::IssetOf(x)
            | Expr::EmptyOf(x)
            | Expr::Quiet(x)
            | Expr::Suppress(x)
            | Expr::VarVar(x) => self.expr(x),
            Expr::IncDec { target, .. } => self.expr(target),
            Expr::Call(_, args) | Expr::New(_, args) | Expr::NewAnon { args, .. } => {
                self.exprs(args)
            }
            Expr::StaticCall(class, _, args) => {
                if let Some(op) = class.operand() {
                    self.expr(op);
                }
                self.exprs(args);
            }
            Expr::CallValue(f, args) => {
                self.expr(f);
                self.exprs(args);
            }
            Expr::MethodCall(r, _, args) | Expr::NullsafeMethodCall(r, _, args) => {
                self.expr(r);
                self.exprs(args);
            }
            Expr::PropGet(r, _) | Expr::NullsafePropGet(r, _) => self.expr(r),
            Expr::Ternary(a, b, c) => {
                self.expr(a);
                self.expr(b);
                self.expr(c);
            }
            Expr::Match { subj, arms } => {
                self.expr(subj);
                for a in arms {
                    for c in a.conds.iter().flatten() {
                        self.expr(c);
                    }
                    self.expr(&a.body);
                }
            }
            Expr::Unset(ts) => self.exprs(ts),
            Expr::Yield { key, value } => {
                if let Some(k) = key {
                    self.expr(k);
                }
                if let Some(v) = value {
                    self.expr(v);
                }
            }
            Expr::Null
            | Expr::Bool(_)
            | Expr::Int(_)
            | Expr::Float(_)
            | Expr::Str(_)
            | Expr::ConstFetch(_)
            | Expr::Magic(_) => {}
            // `$cls::K` / `$cls::$p` / `$cls::m()` — the LEFT of a `::` can be an
            // expression, and it reads a variable. Skipping it (which every one
            // of these arms used to do) hid that read from this analysis, so a
            // variable reached ONLY through `$v::` was promoted into a frame slot
            // and then read as null by any detached chunk — a `try` body, most
            // visibly: `try { echo $o::class; }` answered
            // `Cannot use "::class" on null`.
            Expr::StaticGet(class, _) | Expr::StaticProp(class, _) => {
                if let Some(op) = class.operand() {
                    self.expr(op);
                }
            }
        }
    }
}

/// Number the promoted names, in the order [`promotable`] settled on.
pub fn slot_map(names: &[String]) -> FxHashMap<String, u16> {
    names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.clone(), i as u16))
        .collect()
}
