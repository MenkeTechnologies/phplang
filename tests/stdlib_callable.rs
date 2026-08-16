//! End-to-end tests for the `callable` stdlib category (`src/stdlib/callable.rs`).
//! PHP source in, captured `echo` output out. Expected values were cross-checked
//! against PHP 8's reference `php` CLI.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

// ── call_user_func ───────────────────────────────────────────────────────────

#[test]
fn call_user_func_builtin_name() {
    assert_eq!(run("<?php echo call_user_func('strtoupper', 'hi');"), "HI");
}

#[test]
fn call_user_func_multiple_args() {
    assert_eq!(
        run("<?php echo call_user_func('str_repeat', 'ab', 3);"),
        "ababab"
    );
}

#[test]
fn call_user_func_user_function() {
    assert_eq!(
        run("<?php function add($a,$b){return $a+$b;} echo call_user_func('add', 2, 5);"),
        "7"
    );
}

#[test]
fn call_user_func_closure() {
    assert_eq!(
        run("<?php $f = function($x){ return $x * 2; }; echo call_user_func($f, 21);"),
        "42"
    );
}

#[test]
fn call_user_func_arrow_fn() {
    assert_eq!(
        run("<?php $f = fn($x) => $x + 1; echo call_user_func($f, 9);"),
        "10"
    );
}

#[test]
fn call_user_func_no_args() {
    assert_eq!(
        run("<?php function greet(){return 'hey';} echo call_user_func('greet');"),
        "hey"
    );
}

// ── call_user_func_array ─────────────────────────────────────────────────────

#[test]
fn call_user_func_array_builtin() {
    assert_eq!(
        run("<?php echo call_user_func_array('str_repeat', ['xy', 2]);"),
        "xyxy"
    );
}

#[test]
fn call_user_func_array_user_function() {
    assert_eq!(
        run("<?php function sum3($a,$b,$c){return $a+$b+$c;} echo call_user_func_array('sum3', [1,2,3]);"),
        "6"
    );
}

#[test]
fn call_user_func_array_closure() {
    assert_eq!(
        run("<?php $f = function($a,$b){ return $a . $b; }; echo call_user_func_array($f, ['foo','bar']);"),
        "foobar"
    );
}

#[test]
fn call_user_func_array_empty_array() {
    assert_eq!(
        run("<?php function pi_ish(){return 'P';} echo call_user_func_array('pi_ish', []);"),
        "P"
    );
}

// ── function_exists ──────────────────────────────────────────────────────────

#[test]
fn function_exists_user_function() {
    assert_eq!(
        run("<?php function myfn(){} echo function_exists('myfn') ? 'y' : 'n';"),
        "y"
    );
}

#[test]
fn function_exists_user_function_case_insensitive() {
    assert_eq!(
        run("<?php function MyFn(){} echo function_exists('myfn') ? 'y' : 'n';"),
        "y"
    );
}

#[test]
fn function_exists_known_builtin() {
    assert_eq!(
        run("<?php echo function_exists('strlen') ? 'y' : 'n';"),
        "y"
    );
}

#[test]
fn function_exists_known_builtin_case_insensitive() {
    assert_eq!(
        run("<?php echo function_exists('STRLEN') ? 'y' : 'n';"),
        "y"
    );
}

#[test]
fn function_exists_unknown() {
    assert_eq!(
        run("<?php echo function_exists('no_such_function_xyz') ? 'y' : 'n';"),
        "n"
    );
}

// Expanded builtin coverage: names implemented across the stdlib categories now
// report true (previously only a small curated set did).
#[test]
fn function_exists_expanded_builtins() {
    let names = [
        "array_key_first",
        "ctype_alpha",
        "mb_strlen",
        "str_word_count",
        "hash_hmac",
        "preg_quote",
        "get_debug_type",
        "filter_var",
        "is_subclass_of",
        "fdiv",
    ];
    for n in names {
        assert_eq!(
            run(&format!("<?php echo function_exists('{n}') ? 'y' : 'n';")),
            "y",
            "function_exists('{n}') should be true"
        );
    }
}

// HARDEN: `isset`/`empty` are PHP language constructs, not functions — real PHP
// `function_exists` returns false for them. The curated list previously claimed
// they existed.
#[test]
fn function_exists_language_constructs_are_false() {
    assert_eq!(run("<?php echo function_exists('isset') ? 'y' : 'n';"), "n");
    assert_eq!(run("<?php echo function_exists('empty') ? 'y' : 'n';"), "n");
}

// ── array callables ──────────────────────────────────────────────────────────

const GREETER: &str = "<?php class Greeter {
    public $name = 'world';
    public function hello($p) { return \"hi $p from \" . $this->name; }
    public static function shout($x) { return strtoupper($x) . '!'; }
}";

#[test]
fn call_user_func_instance_method_array() {
    assert_eq!(
        run(&format!(
            "{GREETER} $g = new Greeter(); echo call_user_func([$g, 'hello'], 'bob');"
        )),
        "hi bob from world"
    );
}

#[test]
fn call_user_func_array_instance_method_array() {
    assert_eq!(
        run(&format!(
            "{GREETER} $g = new Greeter(); echo call_user_func_array([$g, 'hello'], ['ann']);"
        )),
        "hi ann from world"
    );
}

#[test]
fn call_user_func_static_method_array() {
    assert_eq!(
        run(&format!(
            "{GREETER} echo call_user_func(['Greeter', 'shout'], 'hey');"
        )),
        "HEY!"
    );
}

#[test]
fn call_user_func_array_static_method_array() {
    assert_eq!(
        run(&format!(
            "{GREETER} echo call_user_func_array(['Greeter', 'shout'], ['ok']);"
        )),
        "OK!"
    );
}

#[test]
fn call_user_func_static_method_string() {
    assert_eq!(
        run(&format!(
            "{GREETER} echo call_user_func('Greeter::shout', 'yo');"
        )),
        "YO!"
    );
}

// The instance element binds `$this`, so mutated/other instances stay distinct.
#[test]
fn call_user_func_instance_method_uses_correct_receiver() {
    let src = format!(
        "{GREETER} $a = new Greeter(); $a->name = 'A'; $b = new Greeter(); $b->name = 'B'; \
         echo call_user_func([$a, 'hello'], 'x'), '|', call_user_func([$b, 'hello'], 'y');"
    );
    assert_eq!(run(&src), "hi x from A|hi y from B");
}

// An array callable [$obj, 'method'] resolves an inherited method up the chain.
#[test]
fn call_user_func_inherited_method_array() {
    let src = "<?php class Base { public function tag() { return 'base'; } } \
        class Child extends Base {} \
        $c = new Child(); echo call_user_func([$c, 'tag']);";
    assert_eq!(run(src), "base");
}

// ── callable forms reach every call site ─────────────────────────────────────

/// An object whose class declares `__invoke` is callable everywhere a callback
/// is taken, not only through `call_user_func`.
///
/// Before the forms were unified in `host::call_value`, `$obj(…)` raised
/// "Object of type C is not callable" and — worse — `array_map($obj, …)` judged
/// the callback absent and returned its input UNMAPPED, a wrong answer rather
/// than an error.
#[test]
fn invoke_object_is_callable_everywhere() {
    let cls = "class Twice { public function __invoke($x) { return $x * 2; } }";
    assert_eq!(
        run(&format!("<?php {cls} $c = new Twice; echo $c(21);")),
        "42"
    );
    assert_eq!(
        run(&format!(
            "<?php {cls} $c = new Twice; var_dump(is_callable($c));"
        )),
        "bool(true)\n"
    );
    assert_eq!(
        run(&format!(
            "<?php {cls} $c = new Twice; echo implode(',', array_map($c, [1, 2, 3]));"
        )),
        "2,4,6"
    );
    assert_eq!(
        run(&format!(
            "<?php {cls} $c = new Twice; echo call_user_func($c, 5);"
        )),
        "10"
    );
    // Named arguments reach `__invoke`'s parameter list.
    assert_eq!(
        run(&format!("<?php {cls} $c = new Twice; echo $c(x: 5);")),
        "10"
    );
    // An object WITHOUT `__invoke` stays uncallable.
    assert_eq!(
        run("<?php var_dump(is_callable(new stdClass));"),
        "bool(false)\n"
    );
}

/// `[$obj, "m"]`, `["C", "m"]` and `"C::m"` used to work only inside
/// `call_user_func`; every other entry point either threw "Array callback must
/// have exactly two elements" on a two-element array or silently ignored it.
#[test]
fn array_and_static_callables_reach_every_call_site() {
    let inst = "class C { public function m($v) { return $v * 2; } }";
    let stat = "class S { public static function m($v) { return $v * 3; } }";
    // Direct invocation of a callable held in a variable.
    assert_eq!(
        run(&format!(
            "<?php {inst} $c = new C; $f = [$c, 'm']; echo $f(4);"
        )),
        "8"
    );
    // Sorting and mapping callbacks.
    assert_eq!(
        run(
            "<?php class K { public function cmp($a, $b) { return $a <=> $b; } }
             $k = new K; $x = [3, 1, 2]; usort($x, [$k, 'cmp']); echo implode(',', $x);"
        ),
        "1,2,3"
    );
    assert_eq!(
        run(&format!(
            "<?php {inst} $c = new C; echo implode(',', array_map([$c, 'm'], [1, 2]));"
        )),
        "2,4"
    );
    assert_eq!(
        run(&format!(
            "<?php {stat} echo implode(',', array_map(['S', 'm'], [1, 2]));"
        )),
        "3,6"
    );
    assert_eq!(
        run(&format!(
            "<?php {stat} echo implode(',', array_map('S::m', [1, 2]));"
        )),
        "3,6"
    );
    // array_filter kept EVERY element when it could not read the callback.
    assert_eq!(
        run(
            "<?php class P { public function big($v) { return $v > 1; } }
             $p = new P; echo implode(',', array_filter([1, 2, 3], [$p, 'big']));"
        ),
        "2,3"
    );
}

/// `Closure::fromCallable` accepts the same forms, and the closure it returns
/// keeps the bound `$this`.
#[test]
fn closure_from_callable_accepts_every_form() {
    assert_eq!(
        run(
            "<?php class C { public $v = 7; public function m() { return $this->v; } }
             $c = new C; $f = Closure::fromCallable([$c, 'm']); echo $f();"
        ),
        "7"
    );
    assert_eq!(
        run("<?php class S { public static function m() { return 8; } }
             $f = Closure::fromCallable('S::m'); echo $f();"),
        "8"
    );
    assert_eq!(
        run("<?php $f = Closure::fromCallable('strtoupper'); echo $f('abc');"),
        "ABC"
    );
}
