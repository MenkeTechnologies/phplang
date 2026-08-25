// Differential parity corpus. Each block below is run as `php -r <block>` by
// tests/parity.rs, against both the reference interpreter and phplang, and all
// three observables — stdout, stderr, exit code — must match byte for byte.
//
// Blocks are separated by a line containing exactly `#==#`. Keep every block
// DETERMINISTIC and machine-independent: no rand, no wall clock, no object ids,
// no absolute paths. `-r` is what keeps diagnostics stable, since the reference
// names the script "Command line code" rather than a temp file.
//
// The expected outputs in parity_expected.bin are captured from the reference
// interpreter and MUST only ever be regenerated from it
// (PHPLANG_PARITY_BLESS=1). Editing them to match a phplang answer would turn
// the harness into a mirror of the bug it exists to catch.

// ── loose comparison over the operand types that disagree ───────────────────
$v = [0, 1, -1, "1", "0", "-1", null, [], "php", "", "0.0", "abc", true, false,
      0.0, "1e2", " 1", "1 ", "10", "1abc"];
$n = count($v);
for ($i = 0; $i < $n; $i++) {
    for ($j = 0; $j < $n; $j++) { echo @($v[$i] == $v[$j]) ? "1" : "0"; }
    echo "\n";
}
#==#
// Ordering (<=>) over the same table.
$v = [0, 1, "1", "0", null, "php", "", true, false, 0.0, "1e2", " 1", "1 "];
foreach ($v as $a) { foreach ($v as $b) { echo @($a <=> $b), ","; } echo "\n"; }
#==#
// Numeric strings are compared as NUMBERS, and two integer ones as INTEGERS —
// widening to double first loses everything past 2^53.
var_dump("9223372036854775807" == "9223372036854775806");
var_dump("9223372036854775807" <=> "9223372036854775806");
var_dump(" 1" <=> "1", " 1" <=> "0", "1 " < "10", " 10" > "9", "1e2" == "100");
$a = PHP_INT_MAX; $b = PHP_INT_MAX - 1;
var_dump($a <=> $b, $a == $b, $a > $b, min($a, $b), max($a, $b));
$x = [PHP_INT_MAX, PHP_INT_MAX - 2, PHP_INT_MAX - 1];
sort($x); var_dump($x);
#==#
// NaN is UNORDERED: <=> answers 1, and all four relational operators are false.
// A bool/null operand is decided as a bool first, an array operand outranks.
var_dump(NAN <=> NAN, NAN <=> 1, 1 <=> NAN);
var_dump(NAN < NAN, NAN > NAN, NAN <= NAN, NAN >= NAN, NAN == NAN);
var_dump(NAN < "abc", NAN >= "1", NAN > false, NAN < false, NAN < [], [] > NAN);
var_dump(INF <=> INF, -INF < INF, 1 < 2, 2 <= 2, "a" < "b");
#==#
// Array key coercion. A `null` key is the empty string, NOT the next index.
$a = [];
$a[false] = "f"; $a[true] = "t"; $a["7"] = "s7"; $a["07"] = "s07";
$a["-3"] = "m3"; $a[""] = "e"; $a["1.0"] = "f1";
var_dump($a);
$c = ["x" => 2]; var_dump(array_keys($c));
$d = []; $d[] = 6; $d[9] = 7; $d[] = 8; var_dump(array_keys($d));
#==#
// An array literal longer than one CallBuiltin operand count (a u8) has to be
// emitted in chunks; 200 elements used to truncate to 144 operands.
$a = [];
for ($i = 0; $i < 200; $i++) { $a[] = $i; }
$b = [  0,  1,  2,  3,  4,  5,  6,  7,  8,  9, 10, 11, 12, 13, 14, 15, 16, 17,
       18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35,
       36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53,
       54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71,
       72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89,
       90, 91, 92, 93, 94, 95, 96, 97, 98, 99,100,101,102,103,104,105,106,107,
      108,109,110,111,112,113,114,115,116,117,118,119,120,121,122,123,124,125,
      126,127,128,129,130,131,132,133,134,135,136,137,138,139,140,141,142,143,
      144,145,146,147,148,149,150,151,152,153,154,155,156,157,158,159,160,161,
      162,163,164,165,166,167,168,169,170,171,172,173,174,175,176,177,178,179,
      180,181,182,183,184,185,186,187,188,189,190,191,192,193,194,195,196,197,
      198,199];
var_dump(count($b), array_sum($b), $b === $a, $b[127], $b[128], $b[199]);
$k = ["a"=>1,"b"=>2,"c"=>3,"d"=>4,"e"=>5,"f"=>6,"g"=>7,"h"=>8,"i"=>9,"j"=>10,
      "k"=>11,"l"=>12,"m"=>13,"n"=>14,"o"=>15,"p"=>16,"q"=>17,"r"=>18,"s"=>19,
      "t"=>20,"u"=>21,"v"=>22,"w"=>23,"x"=>24,"y"=>25,"z"=>26];
var_dump(count($k), array_sum($k), implode("", array_keys($k)));
#==#
// foreach by reference: the loop binds the ELEMENT, so a write is visible in
// the same iteration, an unset key is skipped rather than resurrected, and
// after the loop $v is still an alias of the last element.
$a = [1, 2]; foreach ($a as &$v) { $v = 9; echo implode(",", $a), "|"; }
echo "\n";
$a = [1, 2, 3]; foreach ($a as &$v) { if ($v == 2) unset($a[2]); }
unset($v); print_r($a);
$a = [1, 2, 3]; foreach ($a as &$v) {} foreach ($a as $v) {} print_r($a);
$a = [1, 2]; foreach ($a as &$v) {} $v = 99; print_r($a);
$a = [[1, 2], [3, 4]];
foreach ($a as &$r) { foreach ($r as &$c) { $c++; } unset($c); } unset($r);
print_r($a);
$a = [1, 2, 3];
foreach ($a as &$v) { if ($v == 2) continue; $v *= 10; } unset($v); print_r($a);
$a = [1, 2, 3];
foreach ($a as &$v) { if ($v == 2) break; $v *= 10; } unset($v); print_r($a);
function byval(array $x) { foreach ($x as &$v) { $v = 0; } return $x; }
$o = [1, 2]; print_r(byval($o)); print_r($o);
#==#
// array_filter's $mode selects what the callback is handed.
var_dump(array_filter([0, 1, 2, "", null, "a", []]));
var_dump(array_filter([1, 2, 3, 4], fn($x) => $x % 2 == 0));
var_dump(array_filter(["a" => 1, "b" => 2], fn($k) => $k == "a", ARRAY_FILTER_USE_KEY));
var_dump(array_filter(["a" => 1, "b" => 2], fn($v, $k) => $k == "b", ARRAY_FILTER_USE_BOTH));
var_dump(array_filter(["a" => 1], fn($v, $k) => false, ARRAY_FILTER_USE_BOTH));
#==#
// json_encode flags.
var_dump(json_encode([]), json_encode([], JSON_FORCE_OBJECT));
var_dump(json_encode([1, 2], JSON_FORCE_OBJECT), json_encode([1, [2]], JSON_FORCE_OBJECT));
var_dump(json_encode([1, 2], JSON_FORCE_OBJECT | JSON_PRETTY_PRINT));
var_dump(json_encode(["k" => "v"], JSON_PRETTY_PRINT), json_encode("a/b"));
var_dump(json_encode("a/b", JSON_UNESCAPED_SLASHES), json_encode("é"), json_encode("é", JSON_UNESCAPED_UNICODE));
var_dump(json_encode(["a" => NAN]), json_last_error_msg());
var_dump(json_decode('{"a":1,"b":[1,2.5,null,true]}', true));
#==#
// (int) of a double PHP cannot hold WRAPS modulo 2^64 and warns; the same
// value written as a numeric string saturates and says nothing.
var_dump((int) 1e19, (int) 1e20, (int) -1e19, (int) 1e30);
var_dump((int) NAN, (int) INF, (int) -INF);
var_dump((int) 1.9, (int) -1.9, (int) 9.2e18, (int) "1e19", (int) "abc", (int) true);
$a = []; $a[1e19] = "k"; var_dump(array_keys($a));
#==#
// Float -> string: echo uses precision, var_dump/var_export serialize_precision.
$f = [0.1 + 0.2, 1/3, 1e100, 1e-100, 1.0, 100.0, 1e15, 1e16, 1e17, 0.00001,
      0.000001, 1e21, 1e22, -0.0, INF, -INF, PHP_FLOAT_EPSILON];
foreach ($f as $x) { echo $x, "|", var_export($x, true), "|"; var_dump($x); }
#==#
// printf/sprintf conversion and flag coverage.
printf("[%5d][%-5d][%05d][%+d][%+d]\n", 42, 42, 42, 42, -42);
printf("[%5.2f][%.0f][%e][%E][%.3e]\n", 3.14159, 2.5, 12345.6789, 12345.6789, 0.000123);
printf("[%s][%10s][%-10s][%'*10s][%010s]\n", "ab", "ab", "ab", "ab", "ab");
printf("[%b][%o][%x][%X][%c]\n", 255, 255, 255, 255, 65);
printf("[%2\$s-%1\$s][%%][%u]\n", "a", "b", -1);
printf("[%g][%G][%.3g]\n", 0.00001234, 123456789.0, 123456789.0);
var_dump(sprintf("%.10F", 1/3), sprintf("%d", "12abc"), sprintf("%d", 1.9));
var_dump(sprintf("%s", true), sprintf("%s", null), sprintf("%5.1s", "abc"));
var_dump(vsprintf("%s-%s", ["a", "b"]));
#==#
// number_format rounding and separators.
echo number_format(1234.5678), "\n", number_format(1234.5678, 2), "\n";
echo number_format(1234.5678, 2, ",", "."), "\n";
echo number_format(-1234.5678, 3, ".", " "), "\n";
echo number_format(0.5), "|", number_format(1.5), "|", number_format(2.5), "\n";
echo number_format(1234567.891, 2, '.', ''), "\n";
#==#
// substr / str_pad / strpos negative and out-of-range arguments.
var_dump(substr("hello", -3), substr("hello", -3, 2), substr("hello", 1, -1));
var_dump(substr("hello", -10), substr("hello", 10), substr("hello", 0, -10), substr("hello", 2, 0));
var_dump(str_pad("5", 3, "0", STR_PAD_LEFT), str_pad("ab", 7, "xy", STR_PAD_BOTH), str_pad("abc", 2));
var_dump(strpos("hello", "l"), strpos("hello", "z"), strrpos("hello", "l"), strpos("hello", "l", -2));
var_dump(implode(",", [1, 2, 3]), join("-", ["a"]));
var_dump(explode(",", "a,b,c"), explode(",", "a,b,c", 2), explode(",", "a,b,c", -1), explode(",", ""));
#==#
// String helpers whose edge behaviour is easy to get wrong.
var_dump(trim("  x  "), rtrim("xayy", "y"), ltrim("0012", "0"), trim("a..b", "."), trim("[x]", "[]"));
$n = 0; var_dump(str_replace(["a", "b"], ["b", "c"], "ab"), str_replace("a", "b", "aaa", $n), $n);
var_dump(strtr("abc", "ab", "xy"), strtr("hi all", ["hi" => "hello", "all" => "world"]));
var_dump(str_split("abcde", 2), chunk_split("abcd", 2, "-"), strrev("abc"));
var_dump(wordwrap("The quick brown fox", 10, "\n", true), ucwords("hello|world", "|"));
var_dump(str_contains("abc", ""), str_starts_with("abc", ""), strcmp("a", "b"), strcmp("b", "a"));
var_dump(strnatcmp("img12", "img2"), substr_compare("abcde", "bc", 1, 2), substr_count("aaa", "aa"));
#==#
// Integer division and modulo sign rules.
var_dump(intdiv(7, 2), intdiv(-7, 2), intdiv(7, -2), intdiv(-7, -2));
var_dump(7 % 3, -7 % 3, 7 % -3, -7 % -3);
var_dump(fmod(7.5, 2), fmod(-7.5, 2), 7 / 2, 6 / 3, -7 / 2);
var_dump(2 ** 10, 2 ** 0.5, (-8) ** (1/3));
var_dump(PHP_INT_MAX + 1, PHP_INT_MAX * 2, -PHP_INT_MAX - 2);
#==#
// switch uses LOOSE matching; match uses strict.
switch ("1abc") { case 1: echo "one"; break; case "1abc": echo "str"; break; default: echo "def"; }
echo "\n";
switch (0) { case "a": echo "a"; break; case "0": echo "zero"; break; default: echo "d"; }
echo "\n";
echo match (true) { 1 == "1" => "loose", default => "no" }, "\n";
var_dump(isset($undef), empty($undef), $undef ?? "d", null ?? "e", 0 ?: "f");
#==#
// array_merge vs +, and the splice/slice families.
print_r(array_merge([1, 2], [3], ["a" => 1], ["a" => 2]));
print_r([1, 2] + [3, 4, 5]);
print_r(array_replace([1, 2, 3], [1 => "b"], [3 => "d"]));
$a = [1, 2, 3, 4, 5]; print_r(array_splice($a, 1, 2, ["x", "y", "z"])); print_r($a);
$b = [1, 2, 3]; array_splice($b, -1); print_r($b);
print_r(array_slice([1, 2, 3, 4, 5], 1, 2));
print_r(array_slice([1, 2, 3, 4, 5], -2));
print_r(array_slice(["a" => 1, "b" => 2, 3, 4], 1, 2, true));
#==#
// Sorting: flags, key preservation, and the comparator forms.
$a = ["b" => 1, "a" => 2, "c" => 1];
asort($a); print_r($a); arsort($a); print_r($a); ksort($a); print_r($a);
$s = ["10", "9", "1e1", "abc", "2"];
sort($s); print_r($s); rsort($s); print_r($s);
sort($s, SORT_STRING); print_r($s); sort($s, SORT_NUMERIC); print_r($s);
$u = [3, 1, 2]; usort($u, fn($x, $y) => $x <=> $y); print_r($u);
$u = [3, 1, 2]; usort($u, fn($x, $y) => 0); print_r($u);
#==#
// print_r / var_export / var_dump rendering.
print_r([1, 2, ["a" => ["b" => "c"]]]); echo "\n";
var_export([1, 2, ["a" => ["b" => "c"]]]); echo "\n";
var_export(["x" => true, "y" => null, "z" => 1.5]); echo "\n";
print_r("str"); echo "\n"; print_r(1.0); echo "\n";
print_r(true); print_r(false); echo "|\n";
var_dump(print_r([1], true));
var_dump("abc", 1, 1.5, true, null, [1 => [2]]);
#==#
// Casts and truthiness.
var_dump((string) true, (string) false, (string) null, (string) 0.0);
var_dump((int) "abc", (int) "", (float) "1.5e3");
var_dump((bool) "0", (bool) "0.0", (bool) [], (bool) [0], (bool) "");
var_dump((array) "x", (array) null, (array) 1);
var_dump(gettype(1), gettype(1.0), gettype("s"), gettype(true), gettype(null), gettype([]));
var_dump(is_numeric("1e5"), is_numeric(" 1"), is_numeric("1 "), is_numeric("0x1A"), is_numeric(".5"));
#==#
// String offsets.
$s = "hello";
var_dump($s[0], $s[-1], isset($s[9]));
$s[0] = "H"; var_dump($s);
var_dump(str_repeat("ab", 3), ucfirst("hello world"), lcfirst("ABC"));
var_dump(strlen("héllo"), mb_strlen("héllo"), mb_substr("héllo", 1, 2), mb_strtolower("ÀB"));
#==#
// Regex.
var_dump(preg_match('/(\d+)-(\d+)/', '12-34', $m), $m);
var_dump(preg_match_all('/\d/', 'a1b2', $m2), $m2);
var_dump(preg_replace('/\s+/', ' ', 'a   b'));
var_dump(preg_replace_callback('/\d/', fn($m) => $m[0] * 2, 'a1b2'));
var_dump(preg_split('/[\s,]+/', "a, b  c"), preg_quote("a.b*c"), preg_grep('/^a/', ['ab', 'ba']));
#==#
// Diagnostics and fatals keep their exact text and exit status.
echo $undefined_variable;
echo "after\n";
$a = [];
echo $a["missing"];
echo "end\n";
#==#
try { intdiv(1, 0); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { echo 1 % 0; } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { "g" + 1; } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
var_dump(@("5g" + 1));
echo 1/0;
#==#
// min/max: three implementations, and which one runs depends on the SHAPE of
// the call. A direct two-argument call is the frameless one, a spread or a
// dynamic name is the variadic one, and a lone array is zend_hash_minmax.
var_dump(min(1, NAN), min(NAN, 1), max(1, NAN), max(NAN, 1));
var_dump(min(1, 1.0), max(1, 1.0), min(1.0, 1), max(1.0, 1));
var_dump(min([1, NAN, 2]), max([1, NAN, 2]), min([NAN, 1, 2]), max([NAN, 1, 2]));
var_dump(min([1, 2, NAN]), max([1, 2, NAN]));
var_dump(min(1, NAN, 2), max(1, NAN, 2), min(NAN, 1, 2), max(NAN, 1, 2));
var_dump(call_user_func('min', 1, NAN), call_user_func('max', 1, NAN));
$f = 'min'; var_dump($f(1, NAN));
var_dump(min(...[1, NAN]), max(...[1, NAN]));
var_dump(min(PHP_INT_MAX, 1.0), max(PHP_INT_MAX, 1.0));
var_dump(min(2, 1.0), max(2, 1.0), min(1.0, 2), max(1.0, 2));
#==#
// A NaN is unordered against a string, and zend_compare answers 1 whichever
// side it is on — so the comparison is not merely "not less", it is 1 both ways.
var_dump(NAN == "NAN", NAN <=> "1", "1" <=> NAN, NAN <=> "abc");
var_dump(NAN < 1, NAN > 1, NAN == NAN);
#==#
// min/max reject a lone non-array, and an empty array has no answer.
try { min(1); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { max("x"); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { min([]); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
#==#
// A by-reference parameter needs somewhere to write back to. A call result is
// bound to a temporary after a notice; a literal is an error and the arguments
// after it are never evaluated.
function mk() { return [3, 1, 2]; }
function side() { echo "SIDE\n"; return 1; }
sort(mk());
var_dump(array_push(mk(), 9));
try { sort([3, 1, 2]); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { array_push([1], side()); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { usort([3,1,2], fn($a, $b) => $a <=> $b); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { end([1, 2]); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { settype([1], "array"); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { preg_match('/a/', 'a', []); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { str_replace('a', 'b', 'aa', 0); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { sscanf("1 2", "%d %d", 0, 0); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
#==#
// The argument itself is still evaluated before it is rejected.
function boom() { echo "EVALUATED\n"; return 1; }
try { sort([boom()]); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
// Which expressions count as a location is not guessable from the syntax.
$a = [3, 1, 2]; $n = [[3, 1, 2]];
var_dump(sort($a), sort($n[0]), sort(($a)));
try { sort(@$a); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { sort($a ?? []); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { sort(array: [3, 1, 2]); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
var_dump(sort(array: $a));
// PREFER_REF parameters bind a value when no reference is available, silently.
var_dump(array_multisort([3, 1, 2]), extract(['zz' => 1]), current([1, 2]), key([1, 2]));
#==#
// An array mutator on a property must reach the property, and must leave the
// enclosing call's own operands alone while doing it.
class Stack { public $s = [1, 2, 3]; public static $t = [4, 5]; }
$o = new Stack();
var_dump(array_pop($o->s));
var_dump($o->s);
var_dump(array_shift(Stack::$t), Stack::$t);
var_dump(array_splice($o->s, 0, 1), $o->s);
#==#
// Converting an array to a string has no answer, so the reference substitutes
// the text `Array` and warns wherever the conversion happens.
$a = [1, 2];
echo $a, "\n";
echo "p" . $a, "\n";
var_dump((string) $a, strval($a));
echo "v$a\n";
echo sprintf("%s", $a), "\n";
echo implode(",", [[1], [2]]), "\n";
// Reading the array without converting it says nothing.
var_dump($a == "Array", in_array("Array", [$a]), json_encode($a));
#==#
// A NaN has a string form and still warns, because the text does not read back
// as a number. The infinities are the control: they convert silently.
$n = fdiv(0, 0);
echo $n, "\n";
echo "x" . $n, "\n";
var_dump((string) $n, strval($n));
echo sprintf("%s", $n), "\n";
var_dump(implode(",", [$n, 1]));
echo fdiv(1, 0), " ", fdiv(-1, 0), "\n";
var_dump((string) INF, (string) -INF, is_nan($n));
#==#
// An internal function that throws is named in the trace as its own frame; a
// zero-divisor OPERATOR is not, because no function is being called.
function idz() { return intdiv(1, 0); }
function idm() { return intdiv(PHP_INT_MIN, -1); }
function opd() { return 1 / 0; }
function opm() { return 1 % 0; }
foreach (['idz', 'idm', 'opd', 'opm'] as $f) {
    try { $f(); } catch (\Throwable $e) {
        echo get_class($e), ": ", $e->getMessage(), "\n", $e->getTraceAsString(), "\n";
    }
}
