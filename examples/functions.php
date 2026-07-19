<?php
// User functions with positional params and recursion.
function factorial($n) {
    if ($n <= 1) {
        return 1;
    }
    return $n * factorial($n - 1);
}

function fib($n) {
    if ($n <= 1) {
        return $n;
    }
    return fib($n - 1) + fib($n - 2);
}

function isEven($n) {
    return $n % 2 === 0;
}

function greet($name) {
    return "Hi, " . $name . "!";
}

echo factorial(5), "\n";       // 120
echo fib(10), "\n";            // 55
echo isEven(4) ? "even" : "odd", "\n";
echo greet("Ada"), "\n";

// A couple of stdlib calls layered on a user function's result.
echo strtoupper(greet("world")), "\n";
echo strlen(greet("x")), "\n";
