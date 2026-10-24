//! `zend_ast_export` (Zend/zend_ast.c) for the constant expressions a
//! parameter or property default may hold: what `Reflection*::__toString`
//! prints for a default the reference keeps as an AST because it could not
//! evaluate it while compiling (`self::A`, `N\UC + 1`, `new \N\Foo()`).

use crate::ast::{split_ns_fallback, BinOp, ClassRef, Expr, InterpPart, UnOp};

/// What the compiler may substitute while folding a default.
pub struct FoldCtx<'a> {
    /// Constants are substituted: a property default, but not a parameter's,
    /// which `zend_compile_params` compiles with
    /// `ZEND_COMPILE_NO_CONSTANT_SUBSTITUTION` so reflection can name them.
    pub substitute: bool,
    /// The expression of a class constant the compiler can already see
    /// (`zend_try_ct_eval_class_const`): one of the class being compiled, or
    /// of a class declared earlier in the file.
    pub class_const: &'a dyn Fn(&ClassRef, &str) -> Option<Expr>,
}

/// The default's exported source, or `None` when the reference folds it to a
/// value at compile time — that value is printed instead.
pub fn default_text(e: &Expr, ctx: &FoldCtx) -> Option<String> {
    if foldable(e, ctx) {
        return None;
    }
    let mut out = String::new();
    export(&mut out, e, 0);
    Some(out)
}

/// Whether the reference evaluates `e` while compiling: literals, `true` /
/// `false` / `null`, `C::class`, magic constants, operators over those, and —
/// where substitution is on — the engine's own constants named globally and
/// class constants whose values it can already see.
fn foldable(e: &Expr, ctx: &FoldCtx) -> bool {
    match e {
        Expr::Null | Expr::Bool(_) | Expr::Int(_) | Expr::Float(_) | Expr::Str(_) => true,
        Expr::Interp(parts) => parts.iter().all(|p| matches!(p, InterpPart::Lit(_))),
        Expr::Magic(_) => true,
        Expr::ConstFetch(name) => {
            ctx.substitute
                && split_ns_fallback(name).is_none()
                && crate::host::is_predefined_constant(name)
        }
        Expr::StaticGet(class, k) => {
            if k.eq_ignore_ascii_case("class") {
                return !matches!(class, ClassRef::Name(c) if c.eq_ignore_ascii_case("static"));
            }
            ctx.substitute && (ctx.class_const)(class, k).is_some_and(|v| foldable(&v, ctx))
        }
        Expr::Array(elems, _) => elems.iter().all(|el| {
            !el.by_ref
                && !matches!(el.value, Expr::Spread(_))
                && el.key.as_ref().is_none_or(|k| foldable(k, ctx))
                && foldable(&el.value, ctx)
        }),
        Expr::Unary(_, x) => foldable(x, ctx),
        Expr::Binary(_, a, b) | Expr::Elvis(a, b) | Expr::Coalesce(a, b) => {
            foldable(a, ctx) && foldable(b, ctx)
        }
        Expr::Ternary(a, b, c) => foldable(a, ctx) && foldable(b, ctx) && foldable(c, ctx),
        _ => false,
    }
}

/// `zend_ast_export_ex`: `e` printed where the surrounding operator needs
/// priority `priority`, parenthesized when it binds looser.
fn export(out: &mut String, e: &Expr, priority: u32) {
    match e {
        Expr::Null => out.push_str("null"),
        Expr::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Expr::Int(n) => out.push_str(&n.to_string()),
        // `smart_str_append_double(…, EG(precision), true)`.
        Expr::Float(f) => {
            let s = crate::host::php_gcvt(*f, 14);
            let whole = f.is_finite() && !s.contains('.');
            out.push_str(&s);
            if whole {
                out.push_str(".0");
            }
        }
        Expr::Str(s) => export_str(out, s),
        Expr::Interp(parts) => {
            let text: String = parts
                .iter()
                .map(|p| match p {
                    InterpPart::Lit(s) => s.as_str(),
                    InterpPart::Expr(_) => "",
                })
                .collect();
            export_str(out, &text);
        }
        Expr::ConstFetch(name) => match split_ns_fallback(name) {
            Some((qualified, _)) => out.push_str(qualified),
            None => out.push_str(name),
        },
        Expr::StaticGet(class, k) => {
            export_class(out, class);
            out.push_str("::");
            out.push_str(k);
        }
        Expr::New(class, args) => {
            out.push_str("new ");
            export_class(out, &ClassRef::Name(class.clone()));
            out.push('(');
            export_list(out, args);
            out.push(')');
        }
        // The compiled form keeps no spelling: `array(…)` exports as `[…]`.
        Expr::Array(elems, _) => {
            out.push('[');
            for (i, el) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                if let Some(k) = &el.key {
                    export(out, k, 80);
                    out.push_str(" => ");
                }
                if el.by_ref {
                    out.push('&');
                }
                export(out, &el.value, 80);
            }
            out.push(']');
        }
        Expr::Spread(x) => {
            out.push_str("...");
            export(out, x, 0);
        }
        Expr::Unary(op, x) => {
            let sym = match op {
                UnOp::Neg => "-",
                UnOp::Pos => "+",
                UnOp::Not => "!",
                UnOp::BitNot => "~",
            };
            wrap(out, priority > 240, |out| {
                out.push_str(sym);
                export(out, x, 241);
            });
        }
        Expr::Binary(op, a, b) => {
            let (sym, p, pl, pr) = binary(*op);
            wrap(out, priority > p, |out| {
                export(out, a, pl);
                out.push_str(sym);
                export(out, b, pr);
            });
        }
        Expr::Coalesce(a, b) => wrap(out, priority > 110, |out| {
            export(out, a, 111);
            out.push_str(" ?? ");
            export(out, b, 110);
        }),
        Expr::Ternary(a, b, c) => wrap(out, priority > 100, |out| {
            export(out, a, 100);
            out.push_str(" ? ");
            export(out, b, 101);
            out.push_str(" : ");
            export(out, c, 101);
        }),
        Expr::Elvis(a, c) => wrap(out, priority > 100, |out| {
            export(out, a, 100);
            out.push_str(" ?: ");
            export(out, c, 101);
        }),
        // Nothing else is a constant expression a default can hold.
        _ => out.push_str("<expression>"),
    }
}

fn wrap(out: &mut String, parens: bool, body: impl FnOnce(&mut String)) {
    if parens {
        out.push('(');
    }
    body(out);
    if parens {
        out.push(')');
    }
}

fn export_list(out: &mut String, items: &[Expr]) {
    for (i, x) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        export(out, x, 0);
    }
}

/// `zend_ast_export_ns_name` for a class: `self` / `parent` / `static` as
/// written, any other name fully qualified with its leading backslash.
fn export_class(out: &mut String, class: &ClassRef) {
    match class {
        ClassRef::Name(c)
            if ["self", "parent", "static"]
                .iter()
                .any(|k| c.eq_ignore_ascii_case(k)) =>
        {
            out.push_str(c)
        }
        ClassRef::Name(c) => {
            out.push('\\');
            out.push_str(c.trim_start_matches('\\'));
        }
        ClassRef::Expr(x) => export(out, x, 0),
    }
}

/// `zend_ast_export_quoted_str`: single-quoted with `'` and `\` escaped, or —
/// when the string holds a control character — double-quoted the way
/// `zend_ast_export_qstr` spells it.
fn export_str(out: &mut String, s: &str) {
    let bytes = crate::bytes::to_bytes(s);
    if !bytes.iter().any(|&c| c < b' ') {
        out.push('\'');
        for c in s.chars() {
            if c == '\'' || c == '\\' {
                out.push('\\');
            }
            out.push(c);
        }
        out.push('\'');
        return;
    }
    let mut q = Vec::with_capacity(bytes.len() + 2);
    q.push(b'"');
    for &c in bytes.iter() {
        match c {
            b'\n' => q.extend_from_slice(b"\\n"),
            b'\r' => q.extend_from_slice(b"\\r"),
            b'\t' => q.extend_from_slice(b"\\t"),
            0x0c => q.extend_from_slice(b"\\f"),
            0x0b => q.extend_from_slice(b"\\v"),
            0x1b => q.extend_from_slice(b"\\e"),
            c if c < b' ' => q.extend_from_slice(&[b'\\', b'0', b'0' + c / 8, b'0' + c % 8]),
            b'"' | b'$' | b'\\' => q.extend_from_slice(&[b'\\', c]),
            c => q.push(c),
        }
    }
    q.push(b'"');
    out.push_str(&crate::bytes::from_bytes(&q));
}

/// `BINARY_OP(op, p, pl, pr)` for each operator, from `zend_ast_export_ex`.
fn binary(op: BinOp) -> (&'static str, u32, u32, u32) {
    match op {
        BinOp::Add => (" + ", 200, 200, 201),
        BinOp::Sub => (" - ", 200, 200, 201),
        BinOp::Mul => (" * ", 210, 210, 211),
        BinOp::Div => (" / ", 210, 210, 211),
        BinOp::Mod => (" % ", 210, 210, 211),
        BinOp::Pow => (" ** ", 250, 251, 250),
        BinOp::Concat => (" . ", 185, 185, 186),
        BinOp::Shl => (" << ", 190, 190, 191),
        BinOp::Shr => (" >> ", 190, 190, 191),
        BinOp::BitOr => (" | ", 140, 140, 141),
        BinOp::BitAnd => (" & ", 160, 160, 161),
        BinOp::BitXor => (" ^ ", 150, 150, 151),
        BinOp::StrictEq => (" === ", 170, 171, 171),
        BinOp::StrictNe => (" !== ", 170, 171, 171),
        BinOp::LooseEq => (" == ", 170, 171, 171),
        BinOp::LooseNe => (" != ", 170, 171, 171),
        BinOp::Lt => (" < ", 180, 181, 181),
        BinOp::Le => (" <= ", 180, 181, 181),
        BinOp::Gt => (" > ", 180, 181, 181),
        BinOp::Ge => (" >= ", 180, 181, 181),
        BinOp::Spaceship => (" <=> ", 180, 181, 181),
        BinOp::And => (" && ", 130, 130, 131),
        BinOp::Or => (" || ", 120, 120, 121),
    }
}
