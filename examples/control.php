<?php
// if / elseif / else with comparison and logical operators.
$x = 7;
if ($x > 10 && $x < 20) {
    echo "a";
} elseif ($x > 5 or $x < 0) {
    echo "b";
} else {
    echo "c";
}
echo "\n";

echo ($x % 2 == 0) ? "even" : "odd", "\n";

// while with continue/break.
$i = 0;
$out = "";
while ($i < 5) {
    $i++;
    if ($i == 3) {
        continue;
    }
    if ($i > 4) {
        break;
    }
    $out .= $i;
}
echo "$out\n";

// for loop.
for ($j = 0; $j < 5; $j++) {
    if ($j == 3) {
        break;
    }
    echo $j;
}
echo "\n";

// strict vs loose comparison.
echo (1 == "1") ? "y" : "n";
echo (1 === "1") ? "y" : "n";
echo "\n";
