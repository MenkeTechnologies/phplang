//! Function-call intercepts (aspect-oriented advice) — a glob-matched registry of
//! before/after/around hooks keyed by function-name pattern.
//!
//! This is the substrate for phplang's AOP layer (the same design as the sibling
//! frontends): register a pattern like `"user_*"` or `"*_test"` with an advice
//! kind, and the dispatcher can consult `matches()` to weave advice around a
//! call. The registry and glob matching are live and tested here; wiring it into
//! the `CALL` dispatch loop is a later wave so the fast path stays fast until the
//! feature is turned on explicitly.

use std::cell::RefCell;

/// When advice runs relative to the intercepted call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advice {
    Before,
    After,
    Around,
}

struct Intercept {
    pattern: String,
    advice: Advice,
    handler: String,
}

thread_local! {
    static INTERCEPTS: RefCell<Vec<Intercept>> = const { RefCell::new(Vec::new()) };
}

/// Register `handler` to run as `advice` whenever a called function name matches
/// the glob `pattern`.
pub fn register(pattern: &str, advice: Advice, handler: &str) {
    INTERCEPTS.with(|i| {
        i.borrow_mut().push(Intercept {
            pattern: pattern.to_string(),
            advice,
            handler: handler.to_string(),
        })
    });
}

/// Clear all registered intercepts.
pub fn clear() {
    INTERCEPTS.with(|i| i.borrow_mut().clear());
}

/// The advice handlers whose pattern matches `func`, in registration order.
pub fn matches(func: &str) -> Vec<(Advice, String)> {
    INTERCEPTS.with(|i| {
        i.borrow()
            .iter()
            .filter(|iv| glob_match(&iv.pattern, func))
            .map(|iv| (iv.advice, iv.handler.clone()))
            .collect()
    })
}

/// Whether any intercept is registered (dispatch fast-path guard).
pub fn any() -> bool {
    INTERCEPTS.with(|i| !i.borrow().is_empty())
}

/// A minimal glob matcher supporting `*` (any run) and `?` (one char).
fn glob_match(pat: &str, s: &str) -> bool {
    let (p, t) = (pat.as_bytes(), s.as_bytes());
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (usize::MAX, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = pi;
            mark = ti;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_and_ordering() {
        clear();
        register("user_*", Advice::Before, "log_entry");
        register("*_test", Advice::Around, "guard");
        assert!(any());

        let m = matches("user_create");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].0, Advice::Before);

        let m = matches("unit_test");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].1, "guard");

        assert!(matches("plain").is_empty());
        clear();
    }
}
