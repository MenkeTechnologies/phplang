//! The opcode-number invariant.
//!
//! Every `host::ops` constant is a key into fusevm's builtin table, and
//! `register_builtin` takes the LAST registration for a number. Two constants
//! sharing a number therefore does not fail to build, does not warn, and does
//! not panic — one handler silently shadows the other and the shadowed
//! construct starts doing something else entirely. That is the worst possible
//! failure shape, so it is pinned here.
//!
//! This has bitten for real: `INDEX_ISSET` was added as 105 while `LIST_ELEM_GET`
//! already held 105, and `isset($a['k'])` quietly began dispatching to the
//! destructuring read — warning "Undefined array key" and evaluating to null
//! instead of false.
//!
//! Read out of the source rather than from a hand-kept list, because a
//! hand-kept list is exactly the thing that goes stale and stops catching this.

/// `(name, number)` for every `pub const NAME: u16 = N;` in the `ops` module.
fn opcode_constants() -> Vec<(String, u16)> {
    let src = include_str!("../src/host.rs");
    let body = src
        .split_once("pub mod ops {")
        .expect("host.rs declares `pub mod ops`")
        .1;
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        // Stop at the end of the module — the first line that closes it at
        // column 0 in the original source.
        let Some(rest) = line.strip_prefix("pub const ") else {
            continue;
        };
        let Some((name, tail)) = rest.split_once(": u16 = ") else {
            continue;
        };
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse::<u16>() {
            out.push((name.to_string(), n));
        }
    }
    out
}

#[test]
fn every_opcode_number_is_used_by_exactly_one_constant() {
    let ops = opcode_constants();
    // A parse that finds nothing would pass vacuously, which would be worse than
    // no test at all.
    assert!(
        ops.len() > 50,
        "expected to read the whole ops module, found {} constants",
        ops.len()
    );
    let mut by_number: Vec<(u16, Vec<String>)> = Vec::new();
    for (name, n) in ops {
        match by_number.iter_mut().find(|(num, _)| *num == n) {
            Some((_, names)) => names.push(name),
            None => by_number.push((n, vec![name])),
        }
    }
    let clashes: Vec<String> = by_number
        .iter()
        .filter(|(_, names)| names.len() > 1)
        .map(|(n, names)| format!("{n} => {}", names.join(", ")))
        .collect();
    assert!(
        clashes.is_empty(),
        "opcode numbers used by more than one constant (the later registration \
         silently shadows the earlier): {}",
        clashes.join("; ")
    );
}
