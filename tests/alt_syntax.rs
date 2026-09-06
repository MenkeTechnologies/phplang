//! PHP's alternative control-structure syntax — `if (…): … endif;` and the
//! `endwhile`/`endfor`/`endforeach`/`endswitch`/`enddeclare` family.
//!
//! Every one of the six was a parse error (`syntax error, unexpected token ":"`)
//! before this suite. It is the spelling PHP templates are written in, so the
//! interleaving cases matter as much as the plain ones. Outputs are byte-verified
//! against PHP 8.5.10.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn if_else_elseif_in_alternative_syntax() {
    assert_eq!(run(r#"<?php if (true): echo "t"; endif;"#), "t");
    assert_eq!(
        run(r#"<?php if (false): echo "t"; else: echo "f"; endif;"#),
        "f"
    );
    assert_eq!(
        run(r#"<?php if (false): echo "t"; elseif (true): echo "e"; else: echo "f"; endif;"#),
        "e"
    );
    // An `if` with no matching branch falls through to whatever follows.
    assert_eq!(
        run(r#"<?php if (false): echo "t"; elseif (false): echo "e"; endif; echo "after";"#),
        "after"
    );
}

#[test]
fn loops_in_alternative_syntax() {
    assert_eq!(
        run(r#"<?php for ($i = 0; $i < 3; $i++): echo $i; endfor;"#),
        "012"
    );
    assert_eq!(
        run(r#"<?php $i = 0; while ($i < 3): echo $i; $i++; endwhile;"#),
        "012"
    );
    assert_eq!(
        run(r#"<?php foreach (["a" => 1, "b" => 2] as $k => $v): echo "$k$v"; endforeach;"#),
        "a1b2"
    );
    // `break`/`continue` reach out of an alternative-syntax body as usual.
    assert_eq!(
        run(
            r#"<?php for ($i = 0; $i < 4; $i++): if ($i == 1): continue; endif; if ($i == 3): break; endif; echo $i; endfor;"#
        ),
        "02"
    );
}

#[test]
fn switch_and_declare_in_alternative_syntax() {
    assert_eq!(
        run(
            r#"<?php switch (2): case 1: echo "one"; case 2: echo "two"; default: echo "d"; endswitch;"#
        ),
        "twod"
    );
    assert_eq!(run(r#"<?php declare(ticks=1): echo "d"; enddeclare;"#), "d");
}

#[test]
fn alternative_syntax_survives_a_closing_tag() {
    // The reason the spelling exists: the body can be cut in half by `?> … <?php`
    // and the block still closes.
    assert_eq!(
        run(r#"<?php $x = 1; if ($x): ?>YES<?php else: ?>NO<?php endif;"#),
        "YES"
    );
    assert_eq!(
        run(r#"<?php foreach ([1, 2] as $v): ?>[<?php echo $v; ?>]<?php endforeach;"#),
        "[1][2]"
    );
}

#[test]
fn alternative_bodies_nest() {
    let src = r#"<?php
        foreach ([1, 2] as $v):
            if ($v == 1): echo "one";
            else: echo "other";
            endif;
        endforeach;"#;
    assert_eq!(run(src), "oneother");
}

#[test]
fn a_missing_terminator_is_a_parse_error() {
    // `endif` is not optional: the reference refuses the program rather than
    // treating the `:` body as a single statement.
    let err = eval_capture(r#"<?php if (true): echo 1;"#).unwrap_or_else(|e| e.to_string());
    assert!(err.contains("syntax error"), "got {err:?}");
}
