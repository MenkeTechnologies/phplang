//! Output buffering (`ob_*`) and variadic call introspection
//! (`func_get_args`/`func_num_args`/`func_get_arg`).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn ob_start_captures_echo() {
    let src = r#"<?php
        ob_start();
        echo "buffered";
        $c = ob_get_clean();
        echo "[", $c, "]";"#;
    // The echo goes to the buffer, not stdout, until ob_get_clean returns it.
    assert_eq!(run(src), "[buffered]");
}

#[test]
fn ob_get_contents_without_clearing() {
    let src = r#"<?php
        ob_start();
        echo "abc";
        $peek = ob_get_contents();
        echo "def";
        $all = ob_get_clean();
        echo $peek, "|", $all;"#;
    assert_eq!(run(src), "abc|abcdef");
}

#[test]
fn nested_output_buffers() {
    let src = r#"<?php
        ob_start();
        echo "outer-";
        ob_start();
        echo "inner";
        $lvl = ob_get_level();
        $inner = ob_get_clean();
        echo "[got:", $inner, ",lvl:", $lvl, "]";
        echo ob_get_clean();"#;
    // Outer buffer accumulates "outer-" + the echoed inner report; the final
    // ob_get_clean returns it to the (test) capture.
    assert_eq!(run(src), "outer-[got:inner,lvl:2]");
}

#[test]
fn ob_end_flush_sends_down_a_level() {
    let src = r#"<?php
        ob_start();
        echo "flushed";
        ob_end_flush();
        echo "-tail";"#;
    // ob_end_flush writes the buffer to the level below (here the capture).
    assert_eq!(run(src), "flushed-tail");
}

#[test]
fn ob_end_clean_discards() {
    let src = r#"<?php
        ob_start();
        echo "dropped";
        ob_end_clean();
        echo "kept";"#;
    assert_eq!(run(src), "kept");
}

#[test]
fn func_get_args_returns_all() {
    let src = r#"<?php
        function collect() { return implode(",", func_get_args()); }
        echo collect(1, 2, 3);"#;
    assert_eq!(run(src), "1,2,3");
}

#[test]
fn func_num_args_and_get_arg() {
    let src = r#"<?php
        function info($a) {
            return func_num_args() . ":" . func_get_arg(2);
        }
        echo info("x", "y", "z");"#;
    assert_eq!(run(src), "3:z");
}

#[test]
fn func_get_args_beyond_declared_params() {
    // Extra arguments past the declared parameters are still captured.
    let src = r#"<?php
        function sum() {
            $t = 0;
            foreach (func_get_args() as $n) { $t += $n; }
            return $t;
        }
        echo sum(1, 2, 3, 4, 5);"#;
    assert_eq!(run(src), "15");
}
