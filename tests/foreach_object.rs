//! foreach over objects: public properties, the Iterator protocol,
//! IteratorAggregate/getIterator, the SPL structures, and iterator_to_array/count.

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn foreach_over_public_properties() {
    let src = r#"<?php class P { public $a = 1; public $b = 2; public $c = 3; }
        foreach (new P as $k => $v) { echo "$k=$v "; }"#;
    assert_eq!(run(src), "a=1 b=2 c=3 ");
}

#[test]
fn foreach_over_user_iterator() {
    let src = r#"<?php
        class Counter {
            public $i = 0; public $max;
            function __construct($m) { $this->max = $m; }
            function rewind() { $this->i = 0; }
            function valid() { return $this->i < $this->max; }
            function current() { return $this->i * 10; }
            function key() { return $this->i; }
            function next() { $this->i = $this->i + 1; }
        }
        foreach (new Counter(4) as $k => $v) { echo "$k:$v "; }"#;
    assert_eq!(run(src), "0:0 1:10 2:20 3:30 ");
}

#[test]
fn foreach_over_iterator_aggregate() {
    let src = r#"<?php
        class Bag { public $items = ["p", "q", "r"]; function getIterator() { return $this->items; } }
        foreach (new Bag as $v) { echo $v; }"#;
    assert_eq!(run(src), "pqr");
}

#[test]
fn foreach_over_spl_structures() {
    let stack = r#"<?php $s = new SplStack; $s->push("a"); $s->push("b"); $s->push("c");
        foreach ($s as $v) { echo $v; }"#;
    assert_eq!(run(stack), "abc");
    let ao = r#"<?php $a = new ArrayObject(["x" => 10, "y" => 20]);
        foreach ($a as $k => $v) { echo "$k=$v "; }"#;
    assert_eq!(run(ao), "x=10 y=20 ");
}

#[test]
fn arrays_still_iterate() {
    let src = r#"<?php $t = 0; foreach ([5, 6, 7] as $k => $v) { $t += $k + $v; } echo $t;"#;
    // (0+5)+(1+6)+(2+7) = 21
    assert_eq!(run(src), "21");
}

#[test]
fn iterator_to_array_and_count() {
    let src = r#"<?php $q = new SplQueue; $q->enqueue(1); $q->enqueue(2); $q->enqueue(3);
        $arr = iterator_to_array($q);
        echo implode(",", $arr), "|", iterator_count($q);"#;
    assert_eq!(run(src), "1,2,3|3");
}
