<?php
// Indexed arrays: literal, append, foreach.
$nums = [10, 20, 30];
$nums[] = 40;
echo "nums: " . implode(",", $nums) . "\n";
echo "count: " . count($nums) . "\n";

$total = 0;
foreach ($nums as $n) {
    $total += $n;
}
echo "sum: $total\n";

// Associative arrays: literal, index read/write, foreach with key.
$user = ["name" => "ada", "age" => 36];
$user["age"] = 37;
foreach ($user as $k => $v) {
    echo "$k=$v ";
}
echo "\n";

// A few array library functions.
echo "keys: " . implode(",", array_keys($user)) . "\n";
echo "has 30? " . (in_array(30, $nums) ? "yes" : "no") . "\n";
echo "range: " . implode(",", range(1, 5)) . "\n";
echo "max/min: " . max($nums) . "/" . min($nums) . "\n";
