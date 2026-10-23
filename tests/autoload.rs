//! The `spl_autoload_register` chain and every lookup that consults it.
//!
//! Expected strings are `php -r` output from php 8.5.11 (`/opt/homebrew/bin/php`);
//! no PHP is needed to run them. Each loader echoes the name it was asked for,
//! so the order and the set of autoload calls are part of what is pinned.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn new_static_members_and_constants_autoload_their_class() {
    let src = r#"<?php spl_autoload_register(function ($c) { echo "[$c]";
            eval("class $c { const K = 1; public static \$p = 2; static function m() { return 3; } }"); });
        echo Qa::K, Qb::$p, Qc::m(), get_class(new Qd), get_class(new Qe(1)), "\n";
        Qf::$p = 5; echo Qf::$p, "\n";"#;
    assert_eq!(run(src), "[Qa]1[Qb]2[Qc]3[Qd]Qd[Qe]Qe\n[Qf]5\n");
}

#[test]
fn a_declaration_autoloads_its_parent_and_interfaces() {
    let src = r#"<?php spl_autoload_register(function ($c) { echo "[$c]";
            eval(str_starts_with($c, "I") ? "interface $c {}" : "class $c { function hi() { return 'hi'; } }"); });
        class B extends A implements I1, I2 {}
        echo (new B)->hi(), " ", implode(",", class_implements("B")), "\n";"#;
    assert_eq!(run(src), "[A][I1][I2]hi I1,I2\n");
}

#[test]
fn a_missing_parent_or_interface_is_an_error_where_the_declaration_runs() {
    let src = r#"<?php
        try { eval("class G extends Nope {}"); } catch (Error $e) { echo $e->getMessage(), "|"; }
        try { eval("interface J extends Nope {}"); } catch (Error $e) { echo $e->getMessage(), "|"; }
        try { eval("class K implements Nope {}"); } catch (Error $e) { echo $e->getMessage(); }"#;
    assert_eq!(
        run(src),
        r#"Class "Nope" not found|Interface "Nope" not found|Interface "Nope" not found"#
    );
}

#[test]
fn the_lookup_functions_autoload_as_their_flags_say() {
    // `class_exists(..., false)` and `is_a` without `$allow_string` do not
    // autoload; a name that cannot be a class is never handed to a loader; the
    // leading `\` is dropped from the name a loader sees.
    let src = r#"<?php spl_autoload_register(function ($c) { echo "[$c]"; });
        var_dump(class_exists("Q1"), class_exists("Q2", false), interface_exists("\\Q3"),
            class_exists("not a name"), is_a("Q4", "Q5"), is_a("Q6", "Q7", true),
            is_subclass_of("Q8", "Q9"), method_exists("Q10", "m"));"#;
    assert_eq!(
        run(src),
        "[Q1][Q3][Q6][Q8][Q10]bool(false)\nbool(false)\nbool(false)\nbool(false)\n\
         bool(false)\nbool(false)\nbool(false)\nbool(false)\n"
    );
}

#[test]
fn a_loader_that_throws_ends_the_lookup_and_one_never_reenters_itself() {
    let src = r#"<?php spl_autoload_register(function ($c) { echo "[$c]"; class_exists($c);
            throw new RuntimeException("cannot load $c"); });
        try { new Missing; } catch (RuntimeException $e) { echo $e->getMessage(), "\n"; }
        echo "after\n";"#;
    assert_eq!(run(src), "[Missing]cannot load Missing\nafter\n");
}

#[test]
fn registration_keeps_one_copy_in_order_and_prepend_goes_first() {
    let src = r#"<?php $a = function ($c) { echo "a"; }; $b = function ($c) { echo "b"; };
        spl_autoload_register($a); spl_autoload_register($b); spl_autoload_register($a);
        spl_autoload_register(function ($c) { echo "p"; }, true, true);
        class_exists("X");
        echo "\n", count(spl_autoload_functions()), var_export(spl_autoload_unregister($b), true),
            var_export(spl_autoload_unregister($b), true), count(spl_autoload_functions()), "\n";
        spl_autoload_register(fn ($c) => null, false);"#;
    assert_eq!(
        run(src),
        "pab\n3truefalse2\n\nNotice: spl_autoload_register(): Argument #2 ($do_throw) has been \
         ignored, spl_autoload_register() will always throw in Command line code on line 7\n"
    );
}

#[test]
fn unserialize_autoloads_an_undeclared_class() {
    let src = r#"<?php spl_autoload_register(function ($c) { echo "[$c]"; if ($c == "Y") eval("class Y { public \$a; }"); });
        echo get_class(unserialize('O:1:"Y":0:{}')), " ", get_class(unserialize('O:1:"Z":0:{}')), "\n";"#;
    assert_eq!(run(src), "[Y]Y [Z]__PHP_Incomplete_Class\n");
}

#[test]
fn a_loader_sees_the_fully_qualified_name() {
    let src = r#"<?php namespace App;
        spl_autoload_register(function ($c) { echo "[$c]"; eval("namespace App\\Models; class User {}"); });
        echo get_class(new Models\User), "\n";"#;
    assert_eq!(run(src), "[App\\Models\\User]App\\Models\\User\n");
}

#[test]
fn a_class_no_loader_supplies_is_still_not_found() {
    let src = r#"<?php spl_autoload_register(function ($c) { echo "[$c]"; });
        try { Nope::m(); } catch (Error $e) { echo $e->getMessage(), "\n"; }
        try { echo Nope2::K; } catch (Error $e) { echo $e->getMessage(), "\n"; }
        try { new Nope3; } catch (Error $e) { echo $e->getMessage(), "\n"; }"#;
    assert_eq!(
        run(src),
        "[Nope]Class \"Nope\" not found\n[Nope2]Class \"Nope2\" not found\n[Nope3]Class \"Nope3\" not found\n"
    );
}

#[test]
fn the_default_spl_autoload_includes_the_lowercased_class_file() {
    // `spl_autoload_register()` with no callback registers `spl_autoload`,
    // which looks for `<lowercased class, \ as />.inc` then `.php` along the
    // include path (`.`, the working directory).
    let dir = std::env::temp_dir().join(format!("phplang_spl_autoload_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("app")).unwrap();
    std::fs::write(
        dir.join("app/box.php"),
        "<?php\nnamespace App;\nclass Box { }\necho \"[loaded box]\";\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("widget.inc"),
        "<?php\nclass Widget {}\necho \"[inc]\";\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("main.php"),
        "<?php\nspl_autoload_register();\nvar_dump(spl_autoload_functions());\n\
         echo get_class(new App\\Box), \"\\n\", get_class(new Widget), \"\\n\";\n\
         var_dump(class_exists(\"Nope\"), spl_autoload_extensions());\n\
         spl_autoload_extensions(\".php\");\nvar_dump(spl_autoload_extensions());\n",
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_php"))
        .arg("main.php")
        .current_dir(&dir)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "array(1) {\n  [0]=>\n  string(12) \"spl_autoload\"\n}\n[loaded box]App\\Box\n[inc]Widget\n\
         bool(false)\nstring(9) \".inc,.php\"\nstring(4) \".php\"\n"
    );
}
