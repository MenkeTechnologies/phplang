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
    Interp(Vec<StrPart>),
    /// A `$name` variable read.
    Var(String),
    /// An array literal: `[k => v, v, ...]` / `array(...)`. A `None` key means
    /// the next auto-increment integer index.
    Array(Vec<(Option<Expr>, Expr)>),
    /// `recv[index]`.
    Index(Box<Expr>, Box<Expr>),
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
    StaticGet(String, String),
    /// `Class::$prop` — static property access (`self::$n`, `C::$x`). A read, an
    /// assignment target, and an `++`/`--` target; storage is shared per declaring
    /// class, so all instances and scopes observe the same value.
    StaticProp(String, String),
    /// `Class::method(args)` — static / scope-resolution method call.
    StaticCall(String, String, Vec<Expr>),
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
        val_var: String,
        by_ref: bool,
        body: Vec<Stmt>,
    },
    /// `function name($a, $b) { body }`.
    Function {
        name: String,
        params: Vec<Param>,
        body: Vec<Stmt>,
    },
    /// `class Name [extends Parent] { ... }`.
    Class(ClassDecl),
    Return(Option<Expr>),
    Break,
    Continue,
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
    /// Whether this is an `interface` (vs a `class`/`trait`).
    pub is_interface: bool,
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
