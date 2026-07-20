//! SPL data-structure classes (prelude) + stdClass + spl_object_id. Array-backed,
//! method-driven; foreach-over-instance is not supported (no object iterators).

use phplang::eval_capture;

fn run(src: &str) -> String {
    eval_capture(src).unwrap_or_else(|e| panic!("eval error for {src:?}: {e}"))
}

#[test]
fn stdclass_dynamic_properties() {
    let src = r#"<?php $o = new stdClass; $o->name = "Ada"; $o->age = 36;
        echo $o->name, ":", $o->age;"#;
    assert_eq!(run(src), "Ada:36");
}

#[test]
fn spl_stack_is_lifo() {
    let src = r#"<?php $s = new SplStack;
        $s->push(1); $s->push(2); $s->push(3);
        echo $s->count(), "|", $s->pop(), $s->pop(), "|", $s->top(), "|", $s->count();"#;
    assert_eq!(run(src), "3|32|1|1");
}

#[test]
fn spl_queue_is_fifo() {
    let src = r#"<?php $q = new SplQueue;
        $q->enqueue("a"); $q->enqueue("b"); $q->enqueue("c");
        echo $q->dequeue(), $q->dequeue(), "|", $q->count();"#;
    assert_eq!(run(src), "ab|1");
}

#[test]
fn spl_fixed_array() {
    let src = r#"<?php $f = new SplFixedArray(3);
        $f->offsetSet(0, "x"); $f->offsetSet(2, "z");
        echo $f->getSize(), $f->offsetGet(0), $f->offsetGet(2),
             $f->offsetExists(2) ? "Y" : "N", $f->offsetExists(3) ? "Y" : "N";"#;
    assert_eq!(run(src), "3xzYN");
}

#[test]
fn array_object_storage() {
    let src = r#"<?php $a = new ArrayObject(["one" => 1]);
        $a->offsetSet("two", 2); $a->append(3);
        echo $a->count(), "|", $a->offsetGet("two"), "|",
             $a->offsetExists("one") ? "Y" : "N";"#;
    assert_eq!(run(src), "3|2|Y");
}

#[test]
fn spl_object_storage_by_identity() {
    let src = r#"<?php $s = new SplObjectStorage;
        $a = new stdClass; $b = new stdClass;
        $s->attach($a, "data"); $s->attach($b);
        echo $s->count(), $s->contains($a) ? "Y" : "N", $s->contains($b) ? "Y" : "N";
        $s->detach($a);
        echo "|", $s->count(), $s->contains($a) ? "Y" : "N";"#;
    assert_eq!(run(src), "2YY|1N");
}

#[test]
fn spl_priority_queue_extracts_highest_first() {
    let src = r#"<?php $pq = new SplPriorityQueue;
        $pq->insert("low", 1); $pq->insert("high", 10); $pq->insert("mid", 5);
        echo $pq->extract(), $pq->extract(), $pq->extract();"#;
    assert_eq!(run(src), "highmidlow");
}

#[test]
fn spl_min_and_max_heap() {
    let min = r#"<?php $h = new SplMinHeap; $h->insert(5); $h->insert(1); $h->insert(3);
        echo $h->extract(), $h->extract(), $h->extract();"#;
    assert_eq!(run(min), "135");
    let max = r#"<?php $h = new SplMaxHeap; $h->insert(5); $h->insert(1); $h->insert(3);
        echo $h->extract(), $h->extract(), $h->extract();"#;
    assert_eq!(run(max), "531");
}

#[test]
fn spl_object_id_is_distinct_per_instance() {
    let src = r#"<?php $a = new stdClass; $b = new stdClass;
        echo spl_object_id($a) === spl_object_id($b) ? "same" : "distinct";
        echo "|", spl_object_id($a) === spl_object_id($a) ? "stable" : "unstable";"#;
    assert_eq!(run(src), "distinct|stable");
}
