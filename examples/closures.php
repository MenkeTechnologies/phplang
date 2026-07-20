<?php
// Arrow function: single-expression body, auto-captures free variables.
$base = 100;
$add = fn($x) => $x + $base;
echo $add(5), "\n";                 // 105

// Anonymous function with an explicit `use` capture (by value).
$factor = 3;
$scale = function ($x) use ($factor) {
    return $x * $factor;
};
echo $scale(4), "\n";               // 12

// Closures are first-class values — pass one to array_map.
$doubled = array_map(fn($n) => $n * 2, [1, 2, 3]);
echo implode(",", $doubled), "\n";  // 2,4,6

// A function returning a closure (a simple adder factory).
function adder($n) {
    return fn($x) => $x + $n;
}
$add10 = adder(10);
echo $add10(7), "\n";               // 17

// Immediately-invoked function expression.
echo (function () { return "iife"; })(), "\n";
