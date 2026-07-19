<?php
// Variable interpolation and concatenation.
$name = "world";
echo "Hello, $name!\n";
$greeting = "Hello" . ", " . $name . "!";
echo "$greeting\n";

// String library functions.
$s = "Hello World";
echo strtolower($s), "\n";
echo strtoupper($s), "\n";
echo ucfirst("php"), "\n";
echo trim("  padded  ") . "|\n";
echo str_repeat("ab", 3), "\n";
echo strrev("abc"), "\n";
echo substr($s, 6), "\n";
echo substr($s, 0, 5), "\n";
echo strpos($s, "World"), "\n";
echo str_replace("World", "PHP", $s), "\n";
echo strlen($s), "\n";
