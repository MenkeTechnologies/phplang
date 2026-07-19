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
    /// `foreach ($arr as [$k =>] $v) { body }`.
    Foreach {
        arr: Expr,
        key_var: Option<String>,
        val_var: String,
        body: Vec<Stmt>,
    },
    /// `function name($a, $b) { body }`.
    Function {
        name: String,
        params: Vec<String>,
        body: Vec<Stmt>,
    },
    Return(Option<Expr>),
    Break,
    Continue,
    /// An empty `;` or a `{ }` block.
    Block(Vec<Stmt>),
}

/// One `case`/`default` label of a `switch` plus its (fall-through) body. `test`
/// is `None` for the `default` label.
#[derive(Debug, Clone)]
pub struct SwitchCase {
    pub test: Option<Expr>,
    pub body: Vec<Stmt>,
}
