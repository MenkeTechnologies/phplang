//! End-to-end tests for generators (`yield`, `yield $k => $v`, `yield from`) and
//! the `Generator` object protocol (`current`/`next`/`key`/`valid`/`send`/`throw`/
//! `getReturn`). Generators run as host-side stackful coroutines, so `yield`
//! suspends the whole VM back to the resumer; these assertions are byte-verified
//! against reference PHP 8.5. Source in, captured `echo` output out.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn foreach_over_a_simple_generator() {
    let src = r#"<?php
        function gen() { yield 1; yield 2; yield 3; }
        foreach (gen() as $v) echo $v;"#;
    assert_eq!(run(src), "123");
}

#[test]
fn foreach_binds_the_yielded_keys() {
    let src = r#"<?php
        function kv() { yield 'a' => 1; yield 'b' => 2; }
        foreach (kv() as $k => $v) echo "$k=$v ";"#;
    assert_eq!(run(src), "a=1 b=2 ");
}

#[test]
fn auto_keys_start_at_zero_and_skip_string_keys() {
    // Un-keyed yields take auto-increment integer keys (0,1,…); a string key does
    // not advance the counter, exactly like array-append.
    let src = r#"<?php
        function keyed() { yield 5; yield 'x' => 6; yield 7; }
        foreach (keyed() as $k => $v) echo "$k:$v ";"#;
    assert_eq!(run(src), "0:5 x:6 1:7 ");
}

#[test]
fn explicit_int_key_advances_the_auto_counter() {
    // `yield 5 => 'a'` sets the next auto key to 6; a lower explicit int does not
    // rewind it — matching PHP array-append semantics.
    let src = r#"<?php
        function mixk() { yield 5 => 'a'; yield 'b'; yield 2 => 'c'; yield 'd'; }
        foreach (mixk() as $k => $v) echo "$k:$v ";"#;
    assert_eq!(run(src), "5:a 6:b 2:c 7:d ");
}

#[test]
fn side_effects_interleave_lazily() {
    // A generator body runs only as far as each `yield`; its echoes interleave
    // with the loop body's, proving iteration is lazy (not eager materialization).
    let src = r#"<?php
        function g() { echo "a"; yield 1; echo "b"; yield 2; echo "c"; }
        foreach (g() as $v) echo $v;"#;
    assert_eq!(run(src), "a1b2c");
}

#[test]
fn current_valid_next_key_with_implicit_priming() {
    // The first `current()` primes the generator (runs to the first yield); `next`
    // advances; `valid` reports completion after the last yield.
    let src = r#"<?php
        function g() { echo "before "; yield 1; echo "middle "; yield 2; echo "after "; }
        $g = g();
        echo "[", $g->current(), "]";
        echo "[", $g->key(), "]";
        $g->next();
        echo "[", $g->current(), "]";
        $g->next();
        echo $g->valid() ? "valid" : "done";"#;
    assert_eq!(run(src), "[before 1][0]middle [2]after done");
}

#[test]
fn send_injects_the_yield_expression_value() {
    let src = r#"<?php
        function echoer() { $x = yield 'first'; echo "got $x "; $y = yield 'second'; echo "got $y "; }
        $g = echoer();
        echo $g->current(), " ";
        $g->send('A');
        echo $g->current(), " ";
        $g->send('B');"#;
    assert_eq!(run(src), "first got A second got B ");
}

#[test]
fn send_on_an_unstarted_generator_primes_then_sends() {
    // PHP first advances to the first yield, then sends the value in — the first
    // yielded value is discarded, and `send` returns the next yield.
    let src = r#"<?php
        function acc() { $t = 0; while (true) { $t += yield $t; } }
        $g = acc();
        $g->current();
        echo $g->send(5), $g->send(3), $g->send(2);"#;
    assert_eq!(run(src), "5810");
}

#[test]
fn get_return_captures_the_body_return_value() {
    let src = r#"<?php
        function g() { yield 1; yield 2; return 99; }
        $g = g();
        foreach ($g as $v) echo $v;
        echo "|", $g->getReturn();"#;
    assert_eq!(run(src), "12|99");
}

#[test]
fn infinite_generator_stops_on_break() {
    let src = r#"<?php
        function nat() { $i = 1; while (true) { yield $i++; } }
        foreach (nat() as $n) { if ($n > 5) break; echo $n; }"#;
    assert_eq!(run(src), "12345");
}

#[test]
fn yield_from_a_generator_preserves_keys_and_return() {
    // `yield from` re-emits the delegate's own keys (not the outer auto counter),
    // and evaluates to the delegate's `return` value.
    let src = r#"<?php
        function inner() { yield 10; yield 20; return 'IR'; }
        function outer() { yield 0; $r = yield from inner(); echo "[r=$r]"; yield 3; }
        foreach (outer() as $k => $v) echo "$k:$v ";"#;
    assert_eq!(run(src), "0:0 0:10 1:20 [r=IR]1:3 ");
}

#[test]
fn yield_from_an_array() {
    let src = r#"<?php
        function fa() { yield 100; yield from [7, 8, 9]; yield 200; }
        foreach (fa() as $k => $v) echo "$k:$v ";"#;
    assert_eq!(run(src), "0:100 0:7 1:8 2:9 1:200 ");
}

#[test]
fn throw_is_catchable_inside_the_body() {
    let src = r#"<?php
        function g() {
            try { yield 1; yield 2; }
            catch (Exception $e) { echo "caught:", $e->getMessage(), " "; yield 99; }
        }
        $g = g();
        echo $g->current(), " ";
        echo $g->throw(new Exception("boom"));"#;
    assert_eq!(run(src), "1 caught:boom 99");
}

#[test]
fn uncaught_body_exception_propagates_to_the_resumer() {
    let src = r#"<?php
        function bad() { yield 1; throw new RuntimeException("fail"); }
        $b = bad();
        $b->current();
        try { $b->next(); } catch (RuntimeException $e) { echo "outer:", $e->getMessage(); }"#;
    assert_eq!(run(src), "outer:fail");
}

#[test]
fn a_generator_method_is_a_generator() {
    let src = r#"<?php
        class Range {
            public function upto($n) { for ($i = 1; $i <= $n; $i++) yield $i; }
        }
        $r = new Range();
        foreach ($r->upto(4) as $v) echo $v;"#;
    assert_eq!(run(src), "1234");
}

#[test]
fn generator_parameters_and_locals_are_isolated_per_frame() {
    // A generator created inside a function keeps its own call frame (bound `$n`
    // and local `$i`) separate from the enclosing function's, across suspensions.
    let src = r#"<?php
        function makeGen($base) {
            $g = (function() use ($base) { yield $base + 1; yield $base + 2; })();
            $sum = 0;
            foreach ($g as $v) $sum += $v;
            return $sum;
        }
        echo makeGen(10);"#;
    assert_eq!(run(src), "23");
}

#[test]
fn a_generator_can_drive_another_generator() {
    // The inner generator resumes on its own coroutine stack while the outer one is
    // itself suspended — nested resume/suspend must not corrupt either frame.
    let src = r#"<?php
        function evens($n) { for ($i = 0; $i < $n; $i++) yield $i * 2; }
        function doubled($n) { foreach (evens($n) as $e) yield $e + 1; }
        foreach (doubled(4) as $v) echo "$v ";"#;
    assert_eq!(run(src), "1 3 5 7 ");
}

#[test]
fn a_closure_can_be_a_generator() {
    let src = r#"<?php
        $g = function() { yield 'x'; yield 'y'; };
        foreach ($g() as $v) echo $v;"#;
    assert_eq!(run(src), "xy");
}
