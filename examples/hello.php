<h1>phplang</h1>
<?php
// Scalars, variables, string interpolation.
$name = "world";
echo "Hello, $name!\n";

// Arithmetic on the fusevm fast path.
$a = 6;
$b = 7;
echo $a * $b, "\n";
echo 10 / 4, "\n";      // 2.5
echo 2 ** 10, "\n";     // 1024

// Arrays: indexed, associative, append.
$nums = [1, 2, 3];
$nums[] = 4;
$user = ["name" => "ada", "age" => 36];
echo $user["name"], " is ", $user["age"], "\n";

// Control flow.
$total = 0;
foreach ($nums as $n) {
    $total += $n;
}
echo "sum = $total\n";

for ($i = 0; $i < 3; $i++) {
    echo "i=$i ";
}
echo "\n";

// Functions.
function factorial($n) {
    if ($n <= 1) {
        return 1;
    }
    return $n * factorial($n - 1);
}
echo "5! = ", factorial(5), "\n";

// A few library functions.
echo strtoupper("done"), " ", count($nums), " ", implode(",", $nums), "\n";
?>
<footer>bye</footer>
