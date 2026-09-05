//! End-to-end tests for PHP 8.1 enums: pure enums (`case`, `->name`, `cases()`,
//! singleton identity), backed enums (`->value`, `from()`, `tryFrom()`), enum
//! methods and constants, and `instanceof UnitEnum`/`BackedEnum`. Outputs are
//! byte-verified against the reference `php`.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn pure_enum_case_name() {
    let src = r#"<?php
        enum Suit { case Hearts; case Spades; }
        echo Suit::Hearts->name;"#;
    assert_eq!(run(src), "Hearts");
}

#[test]
fn pure_enum_case_is_singleton() {
    let src = r#"<?php
        enum Suit { case Hearts; case Spades; }
        $a = Suit::Hearts;
        var_dump($a === Suit::Hearts);
        var_dump($a === Suit::Spades);"#;
    assert_eq!(run(src), "bool(true)\nbool(false)\n");
}

#[test]
fn pure_enum_cases_returns_all_in_order() {
    let src = r#"<?php
        enum Suit { case Hearts; case Spades; case Clubs; }
        echo count(Suit::cases());
        echo "|";
        foreach (Suit::cases() as $c) { echo $c->name, ","; }"#;
    assert_eq!(run(src), "3|Hearts,Spades,Clubs,");
}

#[test]
fn enum_instanceof_self_and_unitenum() {
    let src = r#"<?php
        enum Suit { case Hearts; }
        $h = Suit::Hearts;
        echo ($h instanceof Suit) ? "1" : "0";
        echo ($h instanceof UnitEnum) ? "1" : "0";"#;
    assert_eq!(run(src), "11");
}

#[test]
fn backed_enum_value() {
    let src = r#"<?php
        enum Status: string { case Active = 'active'; case Off = 'off'; }
        echo Status::Active->value;
        echo "|";
        echo Status::Off->value;"#;
    assert_eq!(run(src), "active|off");
}

#[test]
fn backed_enum_from_returns_singleton() {
    let src = r#"<?php
        enum Status: string { case Active = 'active'; case Off = 'off'; }
        var_dump(Status::from('off') === Status::Off);"#;
    assert_eq!(run(src), "bool(true)\n");
}

#[test]
fn backed_enum_tryfrom_miss_is_null() {
    let src = r#"<?php
        enum Status: string { case Active = 'active'; }
        var_dump(Status::tryFrom('nope'));
        var_dump(Status::tryFrom('active') === Status::Active);"#;
    assert_eq!(run(src), "NULL\nbool(true)\n");
}

#[test]
fn int_backed_enum_from() {
    let src = r#"<?php
        enum Level: int { case Low = 1; case High = 10; }
        echo Level::High->value;
        echo "|";
        var_dump(Level::from(1) === Level::Low);"#;
    assert_eq!(run(src), "10|bool(true)\n");
}

#[test]
fn backed_enum_instanceof_backedenum() {
    let src = r#"<?php
        enum Status: string { case Active = 'active'; }
        echo (Status::Active instanceof BackedEnum) ? "1" : "0";"#;
    assert_eq!(run(src), "1");
}

#[test]
fn enum_method_reads_this_value() {
    let src = r#"<?php
        enum Status: string {
            case Active = 'active';
            public function label(): string { return ucfirst($this->value); }
        }
        echo Status::Active->label();"#;
    assert_eq!(run(src), "Active");
}

#[test]
fn enum_method_matches_on_self() {
    let src = r#"<?php
        enum Suit: string {
            case Hearts = 'H';
            case Spades = 'S';
            public function color(): string {
                return match($this) {
                    Suit::Hearts => 'Red',
                    Suit::Spades => 'Black',
                };
            }
        }
        echo Suit::Hearts->color(), Suit::Spades->color();"#;
    assert_eq!(run(src), "RedBlack");
}

#[test]
fn enum_constant() {
    let src = r#"<?php
        enum Suit {
            const Wild = 'joker';
            case Hearts;
        }
        echo Suit::Wild;
        echo "|";
        echo Suit::Hearts->name;"#;
    assert_eq!(run(src), "joker|Hearts");
}

/// `Enum::from()` with no matching case is a real call in the reference's trace
/// — `#0 <file>(<line>): E::from(99)` — and both halves of that name are the
/// DECLARED spelling however the call was written, as is the class the message
/// blames. phplang used to raise the ValueError from the caller's own frame, so
/// the trace was one frame short and echoed the caller's casing back.
#[test]
fn backed_enum_from_miss_names_the_call_in_its_trace() {
    let src = r#"<?php
        enum MyLevel: int { case Low = 1; }
        enum Status: string { case Active = 'active'; }
        try { MYLEVEL::FROM(99); } catch (\ValueError $e) {
            echo $e->getMessage(), "\n", $e->getTraceAsString(), "\n";
        }
        try { Status::from('zz'); } catch (\ValueError $e) {
            echo $e->getMessage(), "\n", $e->getTraceAsString(), "\n";
        }"#;
    assert_eq!(
        run(src),
        "99 is not a valid backing value for enum MyLevel\n\
         #0 Command line code(4): MyLevel::from(99)\n#1 {main}\n\
         \"zz\" is not a valid backing value for enum Status\n\
         #0 Command line code(7): Status::from('zz')\n#1 {main}\n"
    );
}

// ── from()/tryFrom() argument coercion ──────────────────────────────────────
//
// The needle used to be compared as its string rendering with no typing at all,
// so an argument the reference refuses outright found a case, and one it
// narrows found none.

#[test]
fn a_type_the_backing_rejects_is_a_type_error() {
    let src = r#"<?php
        enum E: int { case A = 1; }
        try { E::from("z"); } catch (TypeError $e) { echo $e->getMessage(); }
        echo "|";
        try { E::tryFrom([1]); } catch (TypeError $e) { echo $e->getMessage(); }"#;
    assert_eq!(
        run(src),
        "E::from(): Argument #1 ($value) must be of type int, string given|\
         E::tryFrom(): Argument #1 ($value) must be of type int, array given"
    );
}

#[test]
fn a_string_backed_enum_accepts_an_int_and_reports_it_as_a_string() {
    // The union prefers `int`, so the int becomes the backing string — and the
    // failure message quotes it, because that is the type it was coerced to.
    let src = r#"<?php
        enum E: string { case A = "1"; }
        var_dump(E::from(1) === E::A);
        try { E::from(9); } catch (ValueError $e) { echo $e->getMessage(); }"#;
    assert_eq!(
        run(src),
        "bool(true)\n\"9\" is not a valid backing value for enum E"
    );
}

#[test]
fn a_numeric_string_narrows_for_an_int_backed_enum() {
    let src = r#"<?php
        enum E: int { case A = 1; }
        var_dump(E::from("1") === E::A, E::from(" 1") === E::A, E::from(1.0) === E::A);"#;
    assert_eq!(run(src), "bool(true)\nbool(true)\nbool(true)\n");
}

#[test]
fn a_fractional_float_deprecates_its_narrowing() {
    let src = r#"<?php
        enum E: int { case A = 1; }
        var_dump(E::from(1.5) === E::A);"#;
    let out = run(src);
    assert!(
        out.contains("Implicit conversion from float 1.5 to int loses precision")
            && out.contains("bool(true)"),
        "{out}"
    );
}

#[test]
fn null_is_deprecated_and_reads_as_zero() {
    let src = r#"<?php
        enum E: int { case A = 1; }
        try { E::from(null); } catch (ValueError $e) { echo "|", $e->getMessage(); }"#;
    let out = run(src);
    assert!(
        out.contains(
            "E::from(): Passing null to parameter #1 ($value) of type string|int is deprecated"
        ) && out.contains("|0 is not a valid backing value for enum E"),
        "{out}"
    );
}

#[test]
fn a_pure_enum_has_no_from() {
    let src = r#"<?php
        enum E { case A; }
        try { E::from(1); } catch (Error $e) { echo $e->getMessage(); }"#;
    assert_eq!(run(src), "Call to undefined method E::from()");
}
