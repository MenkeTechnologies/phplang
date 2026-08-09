//! SPL data-structure classes (prelude) + stdClass + spl_object_id, plus the
//! `ArrayAccess` / `Countable` protocols they are built on. Array-backed, and
//! reachable through subscripts and `count()` as well as by method.

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

// ── ArrayAccess ──────────────────────────────────────────────────────────────

/// The four `offsetX` methods a subscript can reach, and which one each syntax
/// picks. `isset` asks `offsetExists` and NOTHING else — it never reads through
/// `offsetGet` — which is why the trace below has no `[get]` next to it.
#[test]
fn array_access_routes_each_subscript_to_its_own_method() {
    let src = r#"<?php class A implements ArrayAccess {
            public array $d = ['k' => 'v'];
            public function offsetExists($o): bool { echo "[exists]"; return isset($this->d[$o]); }
            public function offsetGet($o): mixed { echo "[get]"; return $this->d[$o] ?? null; }
            public function offsetSet($o, $v): void {
                echo "[set:", $o ?? 'NULL', "]";
                if ($o === null) { $this->d[] = $v; } else { $this->d[$o] = $v; }
            }
            public function offsetUnset($o): void { echo "[unset]"; unset($this->d[$o]); }
        }
        $a = new A;
        echo $a['k'];
        var_dump(isset($a['k']));
        $a['n'] = 5;
        $a[] = 6;
        unset($a['k']);
        echo implode(",", $a->d);"#;
    assert_eq!(
        run(src),
        "[get]v[exists]bool(true)\n[set:n][set:NULL][unset]5,6"
    );
}

/// A write through a subscript must not replace the object with an array — the
/// bug this pins is `$o['k'] = 1` silently clobbering `$o`.
#[test]
fn writing_through_a_subscript_keeps_the_object() {
    let src = r#"<?php class A implements ArrayAccess {
            private array $d = [];
            public function offsetExists($o): bool { return isset($this->d[$o]); }
            public function offsetGet($o): mixed { return $this->d[$o] ?? null; }
            public function offsetSet($o, $v): void {
                if ($o === null) { $this->d[] = $v; } else { $this->d[$o] = $v; }
            }
            public function offsetUnset($o): void { unset($this->d[$o]); }
        }
        $a = new A;
        $a['x'] = 1;
        $a[] = 2;
        echo get_debug_type($a), "|", $a['x'], "|", $a[0];"#;
    assert_eq!(run(src), "A|1|2");
}

/// `??` on a subscript asks `offsetExists` first and only reads once it says
/// yes, so a missing key never reaches `offsetGet`.
#[test]
fn coalesce_on_a_subscript_checks_existence_before_reading() {
    let src = r#"<?php class A implements ArrayAccess {
            public function offsetExists($o): bool { return $o === 'here'; }
            public function offsetGet($o): mixed { echo "[get:$o]"; return 'V'; }
            public function offsetSet($o, $v): void {}
            public function offsetUnset($o): void {}
        }
        $a = new A;
        echo $a['here'] ?? 'D', "|", $a['gone'] ?? 'D';"#;
    assert_eq!(run(src), "[get:here]V|D");
}

/// `count()` takes an array or a `Countable` and rejects everything else with a
/// TypeError — it does NOT answer 1 for a plain object the way PHP 5 did.
#[test]
fn count_honors_countable_and_rejects_the_rest() {
    let src = r#"<?php class C implements Countable {
            public function count(): int { return 7; }
        }
        echo count(new C), "|", count([1,2]);"#;
    assert_eq!(run(src), "7|2");
    let bad = r#"<?php class N {}
        try { count(new N); } catch (TypeError $e) { echo $e->getMessage(); }"#;
    assert_eq!(
        run(bad),
        "count(): Argument #1 ($value) must be of type Countable|array, N given"
    );
}

/// `SplFixedArray` is a real object reached through the subscript protocol, and
/// `setSize` truncates rather than just moving a counter.
#[test]
fn spl_fixed_array_subscripts_and_resizes() {
    let src = r#"<?php $fa = new SplFixedArray(3);
        $fa[0] = 'a'; $fa[2] = 'c';
        echo count($fa), "|", $fa->getSize(), "|", implode(",", $fa->toArray());
        $fa->setSize(2);
        echo "|", implode(",", $fa->toArray());
        echo "|", implode(",", SplFixedArray::fromArray([1,2,3])->toArray());"#;
    assert_eq!(run(src), "3|3|a,,c|a,|1,2,3");
}

/// `iterator_to_array` drives a Generator to exhaustion, keys and all — the
/// generator is not an object with an `Iterator` implementation, so it needs its
/// own path.
#[test]
fn iterator_helpers_consume_a_generator() {
    let src = r#"<?php function kv() { yield 'a' => 1; yield 'b' => 2; }
        print_r(iterator_to_array(kv()));
        print_r(iterator_to_array(kv(), false));
        echo iterator_count(kv());"#;
    assert_eq!(
        run(src),
        "Array\n(\n    [a] => 1\n    [b] => 2\n)\nArray\n(\n    [0] => 1\n    [1] => 2\n)\n2"
    );
}
