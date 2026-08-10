//! The PHP abstract syntax tree produced by `parser` and consumed by `compiler`.
//!
//! This is the scaffold subset: scalars, variables, arrays, the common operators,
//! `if`/`while`/`for`/`foreach`, function definition and call, and `echo`/`print`.
//! Classes, traits, namespaces, closures, and references are later waves.

/// A binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat, // .
    // Comparison
    LooseEq,  // ==
    LooseNe,  // != / <>
    StrictEq, // ===
    StrictNe, // !==
    Lt,
    Gt,
    Le,
    Ge,
    Spaceship, // <=>
    // Bitwise
    BitAnd, // &
    BitOr,  // |
    BitXor, // ^
    Shl,    // <<
    Shr,    // >>
    // Logical (short-circuit handled in the compiler)
    And, // && / and
    Or,  // || / or
}

/// A unary prefix operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,    // -x
    Pos,    // +x
    Not,    // !x
    BitNot, // ~x
}

/// One segment of a double-quoted / interpolated string.
#[derive(Debug, Clone, PartialEq)]
pub enum StrPart {
    /// A literal run of text.
    Lit(String),
    /// An interpolated `$name` variable.
    Var(String),
    /// PHP source for an interpolation the lexer cannot resolve to a bare name:
    /// the complex form `{$expr}`, and the simple forms `$a->p` and `$a[k]`.
    /// The lexer only records the text — parsing it is the parser's job.
    Raw(String),
}

/// One segment of a *parsed* interpolated string. The parser turns the lexer's
/// [`StrPart`] list into this: a `Var` becomes the expression that reads it and a
/// `Raw` is parsed, so the compiler sees only literal text and expressions.
#[derive(Debug, Clone)]
pub enum InterpPart {
    Lit(String),
    Expr(Box<Expr>),
}

/// What stands to the left of a `::`.
///
/// PHP accepts both a name known at compile time and a value computed at run
/// time (`$cls::CONST`, `$obj::m()`, `$arr['k']::class`), and the two resolve
/// very differently: a name may be the relative `self`/`parent`/`static`, while
/// a value must turn out to be an object or a class-name string or the access
/// is an `Error`.
#[derive(Debug, Clone)]
pub enum ClassRef {
    /// A bareword class name, or one of `self` / `parent` / `static`. Also the
    /// string-literal form `"C"::K`, which PHP resolves the same way.
    Name(String),
    /// `$expr::` — the class is whatever the expression yields at run time.
    Expr(Box<Expr>),
}

impl ClassRef {
    /// The compile-time name, when there is one. `None` for the dynamic form,
    /// which nothing may resolve, forward or diagnose before it runs.
    pub fn name(&self) -> Option<&str> {
        match self {
            ClassRef::Name(n) => Some(n),
            ClassRef::Expr(_) => None,
        }
    }

    /// The operand expression, when the class is computed. Every walk over an
    /// expression tree has to descend through this — a `$cls::K` reads `$cls`
    /// like any other use of it.
    pub fn operand(&self) -> Option<&Expr> {
        match self {
            ClassRef::Name(_) => None,
            ClassRef::Expr(e) => Some(e),
        }
    }
}

/// An expression.
#[derive(Debug, Clone)]
pub enum Expr {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// A single-quoted string (no interpolation).
    Str(String),
    /// A double-quoted / interpolated string.
    Interp(Vec<InterpPart>),
    /// A `$name` variable read.
    Var(String),
    /// An array literal: `[k => v, v, ...]` / `array(...)`. A `None` key means
    /// the next auto-increment integer index.
    Array(Vec<(Option<Expr>, Expr)>),
    /// `recv[index]`.
    Index(Box<Expr>, Box<Expr>),
    /// One element read of a destructuring assignment — `[$a, $b] = $src` and
    /// the `foreach` patterns, never written by the parser.
    ///
    /// This is NOT `Index`. PHP reads a list element through a different
    /// operation, and the two disagree on every non-array subject: `"ab"[0]` is
    /// `'a'`, but `[$x] = "ab"` warns `Cannot use string as array` and assigns
    /// null. Reusing `Index` here silently turns a diagnostic into a character.
    ListElem(Box<Expr>, Box<Expr>),
    /// `recv[]` — append target, only valid as an assignment LHS.
    Append(Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// Assignment. `op` is `None` for `=`, or the compound arithmetic/concat op
    /// for `+=`, `.=`, etc.
    Assign(Box<Expr>, Option<BinOp>, Box<Expr>),
    /// Pre/post increment/decrement: `++$x`, `$x--`.
    IncDec {
        target: Box<Expr>,
        inc: bool,
        prefix: bool,
    },
    /// A function call `name(args)`.
    Call(String, Vec<Expr>),
    /// Argument unpacking at a call site: `...$arr` splats an array's values as
    /// positional arguments. Only valid inside a `Call`'s argument list.
    Spread(Box<Expr>),
    /// A dynamic call of a callable value — `$f(args)`, `foo()(args)`,
    /// `(function(){...})(args)`. The callee evaluates to a closure handle or a
    /// callable string.
    CallValue(Box<Expr>, Vec<Expr>),
    /// An anonymous function `function(params) use(vars) { body }`. `uses` names
    /// the enclosing variables captured at closure-creation time, each by value
    /// or — for `use (&$v)` — by reference.
    Closure {
        params: Vec<Param>,
        uses: Vec<Capture>,
        body: Vec<Stmt>,
    },
    /// An arrow function `fn(params) => expr` — its body is a single expression
    /// and every free variable is captured by value automatically.
    ArrowFn {
        params: Vec<Param>,
        body: Box<Expr>,
    },
    /// `new Class(args)` — instantiate an object (class name literal, or the
    /// `self`/`parent`/`static` keyword resolved at compile time).
    New(String, Vec<Expr>),
    /// `new class(args) [extends P] [implements I] { members }` — an anonymous
    /// class. The declaration is compiled once, at the point the expression is
    /// lowered, under a generated name; every evaluation of the expression then
    /// instantiates that one class (PHP does the same — a `new class` in a loop
    /// produces instances of a single class, not one class per iteration).
    ///
    /// `line` is the source line of the `class` keyword, which the generated
    /// name embeds the way the reference does.
    NewAnon {
        decl: Box<ClassDecl>,
        args: Vec<Expr>,
        line: u32,
    },
    /// `$obj->prop` — instance property read.
    PropGet(Box<Expr>, String),
    /// `$obj->method(args)` — instance method call.
    MethodCall(Box<Expr>, String, Vec<Expr>),
    /// `$obj?->prop` — nullsafe property read: the receiver is evaluated once and,
    /// if it is null, the access short-circuits to null (the property is never
    /// read). Otherwise it behaves like `PropGet`.
    NullsafePropGet(Box<Expr>, String),
    /// `$obj?->method(args)` — nullsafe method call: short-circuits to null when
    /// the receiver is null (the arguments are not evaluated), else like `MethodCall`.
    NullsafeMethodCall(Box<Expr>, String, Vec<Expr>),
    /// A named call argument `name: value` (PHP 8.0). Only valid inside a call's
    /// argument list; the compiler binds it to the parameter of that name.
    NamedArg(String, Box<Expr>),
    /// `Class::CONST` / `Class::class` — class constant read (also `self::`/`parent::`).
    StaticGet(ClassRef, String),
    /// `Class::$prop` — static property access (`self::$n`, `C::$x`). A read, an
    /// assignment target, and an `++`/`--` target; storage is shared per declaring
    /// class, so all instances and scopes observe the same value.
    StaticProp(ClassRef, String),
    /// `Class::method(args)` — static / scope-resolution method call.
    StaticCall(ClassRef, String, Vec<Expr>),
    /// `clone $o` — a new instance of the same class carrying a copy of the
    /// properties, then `__clone()` if the class defines one. The copy is
    /// shallow in PHP's sense: a nested array is a value and is copied, a
    /// nested object is a handle and stays shared.
    Clone(Box<Expr>),
    /// Ternary `cond ? then : els`.
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    /// Short ternary / elvis `a ?: b` — `a` if truthy, else `b` (evaluates `a`
    /// once).
    Elvis(Box<Expr>, Box<Expr>),
    /// Null coalesce `a ?? b` — `a` unless it is null, then `b`.
    Coalesce(Box<Expr>, Box<Expr>),
    /// PHP 8 `match` expression: strict (`===`) comparison of the subject
    /// against each arm's conditions, returning the matching arm's value.
    Match {
        subj: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    /// `throw e` as a PHP 8 expression (also reached from a `throw e;` statement).
    /// Evaluates `e` to an exception object, records it as the pending throw, and
    /// unwinds the current chunk.
    Throw(Box<Expr>),
    /// A bare constant reference (`PHP_EOL`, `M_PI`, a user `define`d name). At
    /// runtime it resolves against the constant table, falling back to the bare
    /// name as a string when undefined (PHP 7 leniency, minus the notice).
    ConstFetch(String),
    /// `unset($a, $b[$k], …)` — remove each variable or array element. Evaluates
    /// to null (a statement-level construct in PHP).
    Unset(Vec<Expr>),
    /// `$x instanceof ClassName` — true if `$x` is an instance of the class or one
    /// of its ancestors/interfaces.
    InstanceOf(Box<Expr>, String),
    /// `$target = &$source` — bind `target` as a reference alias of `source`.
    RefAssign(Box<Expr>, Box<Expr>),
    /// A read in PHP's "isset mode": the operand of `empty()` and the left operand
    /// of `??`. A missing variable, array element or object property is the
    /// question being asked rather than a mistake, so the read raises no
    /// diagnostic. Evaluates exactly as the wrapped expression does.
    ///
    /// On an object property this is the `__isset`-then-`__get` pair: the class is
    /// asked whether the property is set, and only then for its value.
    Quiet(Box<Expr>),
    /// `@expr` — the error-suppression operator. NOT an isset-mode read, despite
    /// the family resemblance: the operand is evaluated exactly as it would be
    /// without the `@`, and only the DIAGNOSTICS it raises are dropped. The
    /// difference is visible wherever the two modes disagree — `@$o->p` on a class
    /// with `__get` calls `__get` (an isset-mode read would ask `__isset` first),
    /// and `@$o->p` on an unreachable private property still throws, because an
    /// `Error` is not a diagnostic.
    Suppress(Box<Expr>),
    /// One `isset()` argument. Narrower than [`Quiet`](Expr::Quiet): it asks only
    /// whether the target is set and never reads a value, so on an object property
    /// it consults `__isset` and stops there — `isset($o->p)` is true for a class
    /// whose `__isset` says so even when `__get` would return null.
    IssetOf(Box<Expr>),
    /// The `empty()` argument, whose value the `!` around it then tests. Sits
    /// between the other two on object properties: like [`Quiet`](Expr::Quiet) it
    /// wants a VALUE, but like [`IssetOf`](Expr::IssetOf) it will not read one
    /// from a property `__isset` has not vouched for. A class with `__get` and no
    /// `__isset` is therefore `empty()` without `__get` ever being called, while
    /// `$o->p ?? 'd'` on that same class does call it.
    EmptyOf(Box<Expr>),
    /// `yield`, `yield $v`, or `yield $k => $v` — suspend the enclosing generator,
    /// handing a value (and optional key) to the resumer. Evaluates to the value
    /// passed by the next `->send($x)` (null for `->next()`). A function whose body
    /// contains a `yield` is a generator: calling it builds a `Generator` object
    /// instead of running the body.
    Yield {
        key: Option<Box<Expr>>,
        value: Option<Box<Expr>>,
    },
    /// `yield from $iterable` — delegate: re-yield every key/value of `$iterable`
    /// (an array, `Traversable`, or another generator) from the enclosing
    /// generator. Evaluates to the delegated generator's `return` value (null for
    /// an array / a generator with no `return`).
    YieldFrom(Box<Expr>),
}

/// One arm of a `match` expression. `conds` is `None` for the `default` arm;
/// otherwise the subject is compared (`===`) against each condition, and the arm
/// fires if any matches.
#[derive(Debug, Clone)]
pub struct MatchArm {
    pub conds: Option<Vec<Expr>>,
    pub body: Box<Expr>,
}

/// A statement. `line` is the 1-based source line, used for the bytecode dump.
#[derive(Debug, Clone)]
pub struct Stmt {
    pub line: u32,
    pub kind: StmtKind,
}

/// The value target of a `foreach`.
///
/// The destructuring spellings all carry an [`Expr::Array`] — the very node the
/// standalone `[$a, $b] = …` assignment uses as its target — so `foreach` gets
/// keyed elements, holes, and nesting from the one destructuring implementation
/// rather than a parallel copy of it. That shared path is also what makes a
/// too-short element warn before binding null, instead of binding null silently.
#[derive(Debug, Clone)]
pub enum ForeachVal {
    /// `foreach ($a as $v)`.
    Var(String),
    /// `foreach ($a as [$x, $y])` or the equivalent `as list($x, $y)`.
    Pattern(Expr),
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    /// A run of literal text outside `<?php ... ?>` — echoed verbatim.
    InlineHtml(String),
    /// `echo e1, e2, ...;`
    Echo(Vec<Expr>),
    /// An expression evaluated for its side effects.
    Expr(Expr),
    /// `if (cond) { then } elseif ... else { els }`.
    If {
        cond: Expr,
        then: Vec<Stmt>,
        /// `(cond, body)` pairs for each `elseif`.
        elifs: Vec<(Expr, Vec<Stmt>)>,
        els: Option<Vec<Stmt>>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },
    /// `do { body } while (cond);` — the body always runs at least once.
    DoWhile {
        cond: Expr,
        body: Vec<Stmt>,
    },
    /// `switch ($subj) { case A: ...; default: ... }`. Cases compare with loose
    /// (`==`) equality and fall through unless a `break` is hit.
    Switch {
        subj: Expr,
        cases: Vec<SwitchCase>,
    },
    For {
        init: Vec<Expr>,
        cond: Option<Expr>,
        step: Vec<Expr>,
        body: Vec<Stmt>,
    },
    /// `foreach ($arr as [$k =>] [&]$v) { body }`. `by_ref` marks `as &$v`, where
    /// mutating `$v` writes back into the array element.
    Foreach {
        arr: Expr,
        key_var: Option<String>,
        val: ForeachVal,
        by_ref: bool,
        body: Vec<Stmt>,
    },
    /// `function name($a, $b) { body }`.
    Function {
        name: String,
        params: Vec<Param>,
        body: Vec<Stmt>,
        /// `function &f()` — the function returns by reference, so `$r = &f()`
        /// aliases the storage its `return` names rather than copying its value.
        by_ref_return: bool,
    },
    /// `class Name [extends Parent] { ... }`.
    Class(ClassDecl),
    Return(Option<Expr>),
    /// `break [n];` — `n` is how many enclosing loop/switch levels to leave
    /// (1 = the innermost, PHP's default).
    Break(u32),
    /// `continue [n];` — see [`StmtKind::Break`] for the level.
    Continue(u32),
    /// `try { body } catch (T1 | T2 $e) { ... } ... [finally { ... }]`.
    Try {
        body: Vec<Stmt>,
        catches: Vec<CatchArm>,
        finally: Option<Vec<Stmt>>,
    },
    /// An empty `;` or a `{ }` block.
    Block(Vec<Stmt>),
    /// `static $a = 1, $b;` inside a function — each name is bound to a persistent
    /// per-declaration slot whose value survives across calls; the optional
    /// initializer runs only on the first entry.
    StaticLocal(Vec<(String, Option<Expr>)>),
}

/// One `catch (T1 | T2 [$var]) { body }` clause. `types` is the union of caught
/// class names; `var` is the optional bound variable (PHP 8 allows `catch (T)`).
#[derive(Debug, Clone)]
pub struct CatchArm {
    pub types: Vec<String>,
    pub var: Option<String>,
    pub body: Vec<Stmt>,
}

/// One formal parameter of a function definition: its name, an optional default
/// value expression (used when the caller omits the argument), whether it is
/// variadic (`...$rest`, collecting all trailing arguments into an array), and
/// whether it is a promoted constructor property (`public int $x`), which makes
/// `__construct` also assign `$this->name = $name`.
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub default: Option<Expr>,
    pub variadic: bool,
    pub promoted: bool,
    /// `readonly` on a promoted constructor parameter, which declares the
    /// property readonly exactly as a `readonly` member declaration would.
    pub readonly: bool,
    /// `&$x` — a by-reference parameter; the caller's variable is updated to the
    /// parameter's final value when the call returns.
    pub by_ref: bool,
}

/// Member visibility, captured on properties and methods. `Public` is the
/// default when no `public`/`protected`/`private` modifier is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Protected,
    Private,
}

/// A declared class property: its name, optional default initializer, whether it
/// is `static` (class-level, shared storage) and its declared visibility.
#[derive(Debug, Clone)]
pub struct PropDecl {
    pub name: String,
    pub default: Option<Expr>,
    pub is_static: bool,
    pub visibility: Visibility,
    /// `readonly` — writable exactly once, from inside the declaring class or a
    /// subclass, and never again. See [`PhpHost::readonly_write_error`].
    ///
    /// [`PhpHost::readonly_write_error`]: crate::host::PhpHost::readonly_write_error
    pub readonly: bool,
}

/// A parsed class declaration. Single inheritance only; interfaces/traits are
/// parsed-and-discarded in the scaffold.
#[derive(Debug, Clone)]
pub struct ClassDecl {
    pub name: String,
    pub parent: Option<String>,
    /// Interfaces this class implements (or, for an `interface`, the interfaces it
    /// extends). Used by `instanceof`/`is_a`/`catch`.
    pub implements: Vec<String>,
    /// Traits pulled in via `use Trait;` inside the class body; their methods and
    /// properties are merged into the class at compile time.
    pub uses: Vec<String>,
    /// `A::m insteadof B, C;` entries of the `use` adaptation block — the trait
    /// whose `m` wins, and the traits whose `m` is dropped.
    pub trait_insteadof: Vec<TraitInsteadof>,
    /// `A::m as [visibility] [alias];` entries of the `use` adaptation block.
    pub trait_aliases: Vec<TraitAlias>,
    /// Whether this is an `interface` (vs a `class`/`trait`).
    pub is_interface: bool,
    /// `readonly class` (PHP 8.2) — every property the class declares is
    /// readonly, promoted constructor parameters included.
    pub is_readonly: bool,
    /// Whether the class is declared `abstract` (cannot be instantiated directly).
    pub is_abstract: bool,
    /// Whether this is an `enum` (PHP 8.1). An enum compiles like a class whose
    /// `cases` are singleton instances; `implements` gains `UnitEnum` (plus
    /// `BackedEnum` when `enum_backing` is set).
    pub is_enum: bool,
    /// The scalar backing type of a backed enum (`enum E: string`) — `"int"` or
    /// `"string"`. `None` for a pure enum.
    pub enum_backing: Option<String>,
    /// `case Name [= value];` entries of an `enum`, in source order.
    pub cases: Vec<EnumCase>,
    /// `const NAME = expr;` entries, in source order.
    pub consts: Vec<(String, Expr)>,
    /// Property declarations, in source order (instance and static).
    pub props: Vec<PropDecl>,
    pub methods: Vec<Method>,
    /// Names of the `#[…]` attributes written before the declaration, unqualified.
    /// Only `AllowDynamicProperties` changes behaviour today; the rest are carried
    /// so the declaration parses and so reflection has something to report.
    pub attributes: Vec<String>,
}

/// One `A::m insteadof B, C;` conflict resolution inside a `use` block. Without
/// it, two used traits declaring the same method is a fatal error.
#[derive(Debug, Clone)]
pub struct TraitInsteadof {
    /// The trait whose method is kept.
    pub winner: String,
    pub method: String,
    /// The traits whose method of that name is dropped.
    pub losers: Vec<String>,
}

/// One `[A::]m as [visibility] [alias];` adaptation inside a `use` block.
///
/// With an `alias`, the method is *additionally* bound under the new name (the
/// original binding stays); without one, only the visibility of the existing
/// binding changes. `from` is the qualifying trait, or `None` for the
/// unqualified `m as …` form, which is an error when more than one used trait
/// declares `m`.
#[derive(Debug, Clone)]
pub struct TraitAlias {
    pub from: Option<String>,
    pub method: String,
    pub alias: Option<String>,
    pub visibility: Option<Visibility>,
}

/// One `case Name [= value];` of an `enum`. `value` is the backing-value
/// expression for a backed enum, `None` for a pure enum.
#[derive(Debug, Clone)]
pub struct EnumCase {
    pub name: String,
    pub value: Option<Expr>,
}

/// A method of a class. `is_static` is retained but not enforced (a static call
/// still binds `$this` when made from an object context, as PHP does).
#[derive(Debug, Clone)]
pub struct Method {
    pub name: String,
    pub params: Vec<Param>,
    pub body: Vec<Stmt>,
    pub is_static: bool,
    pub visibility: Visibility,
    /// `function &m()` — see `StmtKind::Function::by_ref_return`.
    pub by_ref_return: bool,
}

/// One `case`/`default` label of a `switch` plus its (fall-through) body. `test`
/// is `None` for the `default` label.
#[derive(Debug, Clone)]
pub struct SwitchCase {
    pub test: Option<Expr>,
    pub body: Vec<Stmt>,
}

/// One `use (...)` entry of an anonymous function. A by-value capture copies the
/// variable's value when the closure is created; a by-reference one (`use (&$v)`)
/// shares the enclosing variable itself, so the closure's writes are visible
/// outside it and later writes outside are visible in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Capture {
    pub name: String,
    pub by_ref: bool,
}
