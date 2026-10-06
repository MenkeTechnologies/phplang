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
$a = [3, 1, 2]; $vv = 'a'; $n = [[3, 1, 2]];
var_dump(sort($a), sort($$vv), sort($n[0]), sort(($a)));
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
#==#
// `global $x` is a reference binding, not a copy: the local and the global share
// one cell, so each sees the other's writes, and a global that did not exist yet
// is created by the binding.
$g1 = 1; $g2 = 2;
function g_read() { global $g1; return $g1; }
function g_write() { global $g1; $g1 = 10; }
function g_multi() { global $g1, $g2; return $g1 + $g2; }
function g_fresh() { global $g3; $g3 = 7; }
function g_unset() { global $g1; unset($g1); return isset($g1); }
function g_none() { return isset($g1) ? "set" : "unset"; }
var_dump(g_read());
g_write(); var_dump($g1);
var_dump(g_multi());
g_fresh(); var_dump($g3);
// unset() breaks the ALIAS and leaves the global alone.
var_dump(g_unset(), $g1);
// Without the declaration the name is an ordinary, unrelated local.
var_dump(g_none());
// Closures and methods bind the same way.
function g_closure() { $f = function () { global $g1; return $g1; }; return $f(); }
class GHolder { public function m() { global $g1; return $g1; } }
var_dump(g_closure(), (new GHolder())->m());
// At global scope the declaration has no second frame to bind to.
global $g1;
var_dump($g1);
#==#
// Variable variables: the operand's string value names the variable, so the
// name is not known until it runs. `$$x` nests, `${expr}` takes any expression,
// and what it names is an ordinary variable -- assignable, unsettable, and
// acceptable where a by-reference parameter wants a location.
$a = [3, 1, 2]; $vv = 'a'; $name = 'dyn'; $x = 5; $k = 'vv';
var_dump($$vv, ${$vv}, ${'a'}, $$$k);
$$name = 7; var_dump($dyn);
${'q'} = 8; var_dump($q);
var_dump($$vv[0]);
$c = 'x'; $$c += 10; var_dump($x);
function vv_local() { $loc = 'inner'; $$loc = 42; return $inner; }
var_dump(vv_local());
// A quiet context reads through without the undefined-variable warning.
$miss = 'nope';
var_dump(isset($$vv), isset($$miss), empty($$vv), $$miss ?? "dflt");
unset($$vv); var_dump(isset($a));
// A by-reference parameter binds one silently, and a mutator reaches the real
// variable through it.
$b = [3, 1, 2]; $bn = 'b';
var_dump(sort($$bn)); var_dump($b);
var_dump(array_pop($$bn)); var_dump($b);
var_dump(array_push($$bn, 9)); var_dump($b);
// A closure captures the operand, not the name it computes.
$z = [1]; $zn = 'z';
$f = function () use ($zn, $z) { return $$zn; };
var_dump($f());
#==#
// A closure is an expression, and PHP accepts one as a statement on its own.
// A DECLARATION still needs a name, and `function &g()` is still a declaration.
function () { echo "never\n"; };
fn() => 1;
(function () { echo "invoked\n"; })();
function &byref_ret() { static $v = 1; return $v; }
function named_fn() { return 3; }
var_dump(byref_ret(), named_fn());
#==#
// PHP 8 refuses an argument whose type the parameter does not accept. The line
// is drawn per TYPE, not per function: an array is never a string, but a float
// is.
function ty($f) { try { var_dump($f()); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } }
$a = [1, 2];
ty(fn() => strlen($a));
ty(fn() => strtoupper($a));
ty(fn() => substr($a, 0, 1));
ty(fn() => abs($a));
ty(fn() => sqrt($a));
ty(fn() => trim($a));
ty(fn() => ucfirst($a));
ty(fn() => str_repeat($a, 2));
ty(fn() => explode(",", $a));
ty(fn() => strpos($a, "a"));
ty(fn() => number_format($a));
ty(fn() => round($a));
ty(fn() => array_keys("ab"));
ty(fn() => in_array(1, 5));
ty(fn() => implode(",", 5));
ty(fn() => json_decode($a));
#==#
// The types that DO convert still convert, so the check narrows nothing that
// used to work.
function ty2($f) { try { var_dump($f()); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } }
ty2(fn() => strlen(123));
ty2(fn() => strlen(1.5));
ty2(fn() => strlen(true));
ty2(fn() => intdiv("5", 1));
ty2(fn() => intdiv("5.0", 1));
ty2(fn() => intdiv(" 5 ", 1));
ty2(fn() => intdiv(5.0, 1));
ty2(fn() => sqrt("2"));
ty2(fn() => str_replace(5, "a", "b"));
// A non-numeric string is not an int, and neither is a hex spelling.
ty2(fn() => intdiv("5abc", 1));
ty2(fn() => intdiv("abc", 1));
ty2(fn() => intdiv("0x1A", 1));
// An object stands in for a string only through __toString.
class Str { public function __toString(): string { return "s"; } }
class Plain {}
ty2(fn() => strlen(new Str()));
ty2(fn() => strlen(new Plain()));
ty2(fn() => intdiv(new Plain(), 1));
// null is a deprecation in a scalar parameter and a type error in an array one.
ty2(fn() => strlen(null));
ty2(fn() => sqrt(null));
ty2(fn() => array_keys(null));
// A boolean is named in the message by its VALUE.
ty2(fn() => array_keys(true));
ty2(fn() => array_keys(false));
ty2(fn() => array_keys(1.5));
#==#
// An unterminated construct is not reported as a syntax error at all: the
// reference names the bracket and the line it was OPENED on. The `on line N`
// clause is dropped when that is the line being reported anyway.
var_dump(1);
$a = [1
#==#
// `static` closures: not bound to the `$this` of the method they were written
// in, and no instance may be given to one afterwards.
class SC { public $p = 7;
  public function normal() { $f = function () { return $this->p; }; return $f(); }
  public function stat() { $f = static function () { return isset($this) ? "has" : "none"; }; return $f(); }
}
$sc = new SC();
var_dump($sc->normal(), $sc->stat());
$sf = static fn($x) => $x + 1;
var_dump($sf(1));
$sg = static function ($x) { return $x * 2; };
var_dump($sg(3));
var_dump(Closure::bind($sg, new SC(), SC::class));
// An ordinary closure still binds.
$of = function () { return $this->p; };
var_dump(Closure::bind($of, new SC(), SC::class)());
#==#
// A promoted constructor parameter assigns in the CONSTRUCTOR's frame, so a
// variable of the same name at the call site cannot reach it.
class Prom { public function __construct(public int $b = 0, public int $v = 0) {} }
$b = new Prom(b: 5);
var_dump($b->b, $b->v);
$v = new Prom(3, 4);
var_dump($v->b, $v->v);
class Prom2 { public function __construct(public int $a = 1, public int $b = 2) {} }
$p = new Prom2(b: 9);
var_dump($p->a, $p->b);
#==#
// `getReturn()` before the body has returned is refused, not answered with null
// — the reference cannot tell "returned nothing" from "has not returned yet".
function genr() { yield 1; return 99; }
$g = genr();
try { var_dump($g->getReturn()); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
foreach ($g as $_) {}
var_dump($g->getReturn());
function genr2() { yield 1; }
$h = genr2();
foreach ($h as $_) {}
var_dump($h->getReturn());

#==#

// ── get_defined_vars: the frame's own bound variables, in binding order ──
function f($p) { $a = 1; $b = null; unset($a); return get_defined_vars(); }
print_r(f(7));
function g() { return get_defined_vars(); }
var_dump(count(g()));
function h(...$rest) { return array_keys(get_defined_vars()); }
print_r(h(1, 2));
function k() { extract(["e" => 5]); return get_defined_vars(); }
print_r(k());
function m() { $x = 1; $c = function () use ($x) { return get_defined_vars(); }; return $c(); }
print_r(m());
class C { public function meth() { $z = 1; return array_keys(get_defined_vars()); } }
print_r((new C)->meth());
function n() { $v = null; return [array_key_exists("v", get_defined_vars()), isset($v)]; }
var_dump(n());
function o() { $v = 1; unset($v); return array_key_exists("v", get_defined_vars()); }
var_dump(o());

#==#

// ── static locals, by-reference binding, and closure capture ────────────────
function counter() { static $n = 0; $n++; return $n; }
echo counter(), counter(), counter(), "\n";
function byref(&$x) { $x .= "!"; }
$a = "a"; byref($a); echo $a, "\n";
$b = 1; $c = &$b; $c = 5; echo $b, " ", $c, "\n";
unset($c); $c = 9; echo $b, " ", $c, "\n";
$arr = [1, 2, 3];
foreach ($arr as &$v) { $v *= 2; }
unset($v);
print_r($arr);
$q = 1;
$byref = function () use (&$q) { $q++; };
$byref(); $byref(); echo $q, "\n";
$byval = function () use ($q) { return $q; };
$q = 99; echo $byval(), "\n";
$name = "dyn"; $$name = 42; echo $dyn, "\n";
print_r(compact("dyn"));
$s = 5; extract(["s" => 9, "t" => 2]); echo $s + $t, "\n";
#==#
// ── `...` unpacking: array literals, argument lists, and what cannot unpack ──
// An integer key from a spread is RENUMBERED; a string key is kept, and a later
// one overwrites an earlier.
var_dump([...[1, 2], ...[3, 4]]);
var_dump([...[5 => "a", 9 => "b"]]);
var_dump([...["x" => 1], ...["x" => 2]]);
var_dump([...["a" => 1], ...[7 => 2]]);
var_dump([1, ...[2, 3], 4]);
var_dump([...[1, 2], "k" => 9, ...[3]]);
var_dump([...[]]);
// A literal key written after a spread still wins over the spread's.
$a = ["x" => 1];
var_dump([...$a, "x" => 2]);
// A Generator unpacks by being DRIVEN, in both positions.
function g2() { yield 1; yield 2; }
var_dump([...g2()]);
function sum2(...$n) { return array_sum($n); }
echo sum2(...g2()), "\n";
// So does anything following the Traversable protocols.
class Agg implements IteratorAggregate {
    public function getIterator(): Iterator { return new ArrayIterator([1, 2]); }
}
var_dump([...new Agg]);
// Unpacking something that is neither is an error, and the CLASS depends on
// where it is written and what it is: an array literal says `TypeError` for an
// object and `Error` for a scalar, an argument list says `TypeError` for both.
$obj = new stdClass;
$str = "s";
$nul = null;
$int = 5;
foreach ([$obj, $str, $nul, $int] as $bad) {
    try { $x = [...$bad]; } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
}
foreach ([$obj, $str, $nul, $int] as $bad) {
    try { sum2(...$bad); } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
}
// Variadics collect positionally, and a typed variadic checks each element.
function head2($first, ...$rest) { return $first . ":" . implode(",", $rest); }
echo head2(1, 2, 3), "\n";
echo head2(...[1, 2, 3]), "\n";
function ints2(int ...$n) { return count($n); }
echo ints2(1, 2, 3), "\n";
// Named arguments bind by name, in any order.
function three($a, $b, $c) { return "$a|$b|$c"; }
echo three(1, c: 3, b: 2), "\n";
echo three(a: 1, b: 2, c: 3), "\n";
#==#
// ── a spread's STRING keys are named arguments, its integer keys positional ──
// The key decides, not the position the entry happened to occupy: an array
// listing `c` first still binds `c` to $c.
function three_of($a, $b, $c) { return "$a|$b|$c"; }
echo three_of(...["c" => 3, "a" => 1, "b" => 2]), "\n";
echo three_of(...["a" => 1, "b" => 2, "c" => 3]), "\n";
echo three_of(...[1, 2, 3]), "\n";
// Positional arguments before the spread keep their places.
echo three_of(1, ...["c" => 3, "b" => 2]), "\n";
// A named argument may skip a parameter that has a default.
function with_defaults($a, $b = 9, $c = 8) { return "$a|$b|$c"; }
echo with_defaults(...["a" => 1, "c" => 3]), "\n";
echo with_defaults(...["a" => 1]), "\n";
// An integer-keyed array is positional even when the keys are out of order or
// sparse, because unpacking renumbers them.
echo three_of(...[0 => 1, 1 => 2, 2 => 3]), "\n";
// A variadic collects the positional form.
function how_many(...$n) { return count($n); }
echo how_many(...[1, 2, 3]), "\n";
// A Generator unpacks positionally — it has no string keys to name with.
function gen_two() { yield 1; yield 2; }
function add_all(...$n) { return array_sum($n); }
echo add_all(...gen_two()), "\n";
// Both spellings of the same call agree.
function pair($a, $b) { return "$a$b"; }
var_dump(pair(...["a" => 1, "b" => 2]) === pair(...[1, 2]));
var_dump(pair(...["b" => 2, "a" => 1]) === pair(1, 2));
#==#
// ── the call-shape errors, raised before the body runs ──
// A named argument that names no parameter is an error, not a silent drop —
// unless the function has a variadic, which collects it under its name.
function one_param($a) { return $a; }
try { one_param(zz: 1); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
function with_rest($a, ...$rest) { return count($rest); }
echo with_rest(1, zz: 5), "\n";
// Naming a parameter a positional argument already filled is an error too.
try { one_param(1, a: 2); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
// A required parameter left unfilled is ArgumentCountError, whether the call
// was short positionally or named the wrong subset. "exactly" when every
// parameter is required, "at least" when some are optional.
function two_required($a, $b) { return "$a$b"; }
function one_optional($a, $b, $c = 3) { return "$a$b$c"; }
try { two_required(1); } catch (ArgumentCountError $e) { echo get_class($e), "\n"; }
try { two_required(a: 1); } catch (ArgumentCountError $e) { echo get_class($e), "\n"; }
try { one_optional(1); } catch (ArgumentCountError $e) { echo get_class($e), "\n"; }
// The counts and wording, with the file and line stripped so the record is
// machine-independent.
function tail_of($e) { return preg_replace('/ in .*? on line \d+/', ' in FILE on line N', $e->getMessage()); }
try { two_required(1); } catch (ArgumentCountError $e) { echo tail_of($e), "\n"; }
try { one_optional(1); } catch (ArgumentCountError $e) { echo tail_of($e), "\n"; }
class Holder { public function pair($a, $b) { return "$a$b"; } }
try { (new Holder)->pair(1); } catch (ArgumentCountError $e) { echo tail_of($e), "\n"; }
// Calls that ARE well formed still run, by position, by name, and mixed.
echo two_required(1, 2), " ", two_required(b: 2, a: 1), " ", two_required(1, b: 2), "\n";
function has_default($a, $b = 2) { return "$a$b"; }
echo has_default(a: 9), " ", has_default(1), " ", has_default(1, 5), "\n";
function no_params() { return "ok"; }
echo no_params(), "\n";
#==#
// ── a generator destroyed while suspended runs its `finally` blocks ──
// Leaving a `foreach` over a generator the subject expression created (by
// `break`, `break 2` or `return`) frees it at once; `catch` is not consulted.
function gen_fin($tag) { try { yield 1; yield 2; } catch (Throwable $e) { echo "caught "; } finally { echo "F$tag "; } }
foreach (gen_fin("a") as $v) { echo $v, " "; break; }
echo "after-a\n";
foreach (gen_fin("b") as $v) { foreach ([1] as $x) { break 2; } }
echo "after-b\n";
function first_of() { foreach (gen_fin("c") as $v) { return $v; } }
echo first_of(), " after-c\n";
// One a variable still holds survives the loop and is freed at shutdown,
// after everything the script printed — so is one parked by `->current()`.
$held = gen_fin("d");
foreach ($held as $v) { break; }
echo "after-d\n";
// One run to completion ran its `finally` on the way out; nothing is left.
foreach (gen_fin("e") as $v) {}
echo "after-e\n";
#==#
// A `yield` reached in a `finally` while the generator is being destroyed is
// an Error raised at the destroying site.
function yields_in_finally() { try { yield 1; } finally { echo "fin "; yield 2; } }
foreach (yields_in_finally() as $v) { break; }
echo "unreached\n";
#==#
// `exit` at the top level still frees a parked generator, and the status the
// script asked for survives it.
function parked() { try { yield 1; } finally { echo "freed\n"; } }
$p = parked();
$p->current();
exit(3);
#==#
// ── a property write on something that cannot hold one is an Error ──
// The type is named by `zend_zval_value_name`: a bool by its value. The verb
// follows the write: assign, increment/decrement, or modify for a fetch that
// writes deeper.
function attempt($f) { try { $f(); echo "no error\n"; } catch (Error $e) { echo $e->getMessage(), "\n"; } }
attempt(function () { $x = null; $x->k = 1; });
attempt(function () { $x = true; $x->k = 1; });
attempt(function () { $x = 5; $x->k .= "a"; });
attempt(function () { $x = 1.5; $x->k++; });
attempt(function () { $x = "s"; $x->k[] = 1; });
attempt(function () { $x = []; $x->k = 1; });
attempt(function () { $c = fn() => 1; $c->k = 1; });
attempt(function () { $g = (function () { yield 1; })(); $g->k = 1; });
// A plain `=` fetches a property chain for WRITING: the missing link is
// created as null without an `Undefined property` warning, and the write after
// it is what fails. A compound write reads the link, and warns as a read does.
$o = new stdClass;
attempt(function () use ($o) { $o->inner->k = 5; });
attempt(function () use ($o) { $o->a->b->c = 1; });
attempt(function () use ($o) { $o->x->y[] = 1; });
attempt(function () use ($o) { $o->m->n .= 1; });
attempt(function () use ($o) { $o->r->s++; });
class NoDynamic {}
$d = new NoDynamic;
attempt(function () use ($d) { $d->w->n = 1; });
attempt(function () use ($d) { $d->m->n .= 1; });
print_r($d);
print_r($o);
// An existing object link is written through.
$o->inner = new stdClass;
$o->inner->k = 7;
$o->inner->list[] = 8;
print_r($o->inner);
// `__get` supplies the link; a non-object one cannot carry the write back.
class MagicLink { public function __get($n) { echo "get $n\n"; return $n === "obj" ? new stdClass : null; } }
$m = new MagicLink;
attempt(function () use ($m) { $m->obj->k = 1; });
attempt(function () use ($m) { $m->nul->k = 1; });
// An uninitialized typed property is fetched as null and stays uninitialized
// (print_r leaves an uninitialized property out).
class TypedLink { public stdClass $s; }
$t = new TypedLink;
attempt(function () use ($t) { $t->s->k = 1; });
print_r($t);
#==#
// An undefined variable as the receiver of a plain property write raises no
// `Undefined variable` warning — only the Error.
$undefined->k = 1;
#==#
// ── unpacking a scalar inside a CONSTANT array literal is a compile-time fatal ──
// Nothing before it runs, and no `catch` can see it.
echo "never printed\n";
try { var_dump([1, ..."ab", 2]); } catch (Throwable $e) { echo "caught\n"; }
#==#
// The same spread in a non-constant literal is a catchable Error at run time.
$x = 1;
foreach (['ab', 5, null, true, 1.5] as $bad) {
    try { var_dump([$x, ...$bad]); } catch (Error $e) { echo $e->getMessage(), "\n"; }
}
#==#
function never_called() { return [...false]; }
echo "never printed\n";
#==#
// A `<<<` that does not open a well-formed heredoc header (here, a label that
// begins with a digit) is not a heredoc: the scanner takes `<<` and the parse
// error names that token.
$s = <<<9BAD
v
9BAD;
var_dump($s);
#==#
// ── a by-reference parameter of a method, judged when the argument is sent ──
// The callee is only known at run time, but the verdict is the same as for a
// function: a literal is an Error (and the arguments after it never run), a
// call result binds to a temporary after a notice.
function five() { return 5; }
class ByRefMethods {
    public function bump(&$a, $b = 0) { $a++; return $a; }
    public static function stat(&$a) { return "static"; }
    public function __call($n, $args) { return "magic $n"; }
}
class ByRefChild extends ByRefMethods {}
$o = new ByRefMethods;
function sent($f) { try { var_dump($f()); } catch (Error $e) { echo $e->getMessage(), "\n"; } }
sent(fn() => $o->bump(1, print("never\n")));
sent(fn() => (new ByRefChild)->bump(2));
sent(fn() => ByRefMethods::stat(3));
sent(fn() => $o->bump(1, ...[2]));
sent(fn() => $o->bump(five()));
sent(fn() => $o->undefinedMethod(1));
$name = "bump";
sent(fn() => $o->$name(4));
// A variable is a real location, and is written back — also from a call
// with a spread or named arguments after it.
$x = 1;
$o->bump($x);
$o->bump($x, ...[9]);
$o->bump($x, b: 9);
var_dump($x);
#==#
// A named argument to a user function's by-reference parameter is written
// back to the variable it names, and reads an unset one quietly.
function out_param($v, &$out) { $out = $v * 2; }
out_param(out: $r, v: 4);
var_dump($r);
out_param(5, out: $q);
var_dump($q);
function incr(&$a, $b = 0) { $a++; }
$n = 1;
incr($n, b: 2);
incr(b: 1, a: $n);
var_dump($n);
#==#
// ── the bitwise operators put a constant left operand second, as `*` does ──
// Visible in the operand order of the refusal. `(2.5 | 2.5)` is not folded at
// compile time (folding would have to deprecate), so it is a runtime operand.
$t = [];
foreach (['|', '&', '^'] as $op) {
    try {
        echo match ($op) { '|' => "x" | $t, '&' => "x" & $t, '^' => "x" ^ $t }, "\n";
    } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
}
try { var_dump("INF" | (2.5 | 2.5)); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
try { var_dump($t | "x"); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
var_dump("ab" | "  ", 6 & 3, 5 ^ 1);
#==#
// ── substr_count refuses a window that leaves the haystack ──
foreach ([[0, 1, ""], [0, 5, "abc"], [2, 2, "abc"], [-1, 2, "abc"], [1, -3, "abc"],
          [4, null, "abc"], [-7, null, "abcabc"], [3, 0, "abc"], [-6, -1, "abcabc"],
          [1, -1, "abcabc"]] as [$off, $len, $hay]) {
    try { var_dump(substr_count($hay, "a", $off, $len)); }
    catch (ValueError $e) { echo $e->getMessage(), "\n"; }
}
#==#
// ── EXTR_PREFIX_IF_EXISTS binds a compiled-but-unset variable unprefixed ──
// The function mentions $x and $y later, so both are UNDEF compiled variables
// at the call: each is assigned as itself and counted. $z is never mentioned.
function extr_cv() {
    $a = ["x" => 2, "y" => 3, "z" => 4];
    var_dump(extract($a, EXTR_PREFIX_IF_EXISTS, "p"));
    var_dump(get_defined_vars());
    $x = 9; unset($y);
}
extr_cv();
function extr_unset() {
    $x = 1; unset($x);
    $a = ["x" => 2];
    var_dump(extract($a, EXTR_PREFIX_IF_EXISTS, "p"), $x);
}
extr_unset();
#==#
// ── an unplaceable name loses only to the parameters parsed before the variadic ──
foreach ([
    fn() => sprintf([], x: 1),
    fn() => sprintf("%s", [], x: 1),
    fn() => array_map(null, null, x: 1),
    fn() => array_map(1, [], x: 1),
    fn() => array_map(null, x: 1),
    fn() => array_diff(1, x: 1),
    fn() => array_intersect_key(1, [], x: 1),
    fn() => array_udiff(1, [], "strcmp", x: 1),
    fn() => array_replace(1, x: 1),
    fn() => max([], x: 1),
] as $f) {
    try { $f(); } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
}
#==#
// ── an $encoding html.c does not know is warned about and treated as UTF-8 ──
foreach (["utf8", "latin1", "ab", "UTF-8", "iso-8859-1", "sjis-WIN", "cp932", ""] as $cs) {
    var_dump(htmlspecialchars("<a>", ENT_QUOTES, $cs));
}
var_dump(htmlentities("é", ENT_QUOTES, "ascii"));
var_dump(html_entity_decode("&lt;", ENT_QUOTES, "zz"));
var_dump(htmlspecialchars("x", ENT_QUOTES, null));
#==#
// ── request end: shutdown functions, then destructors ──
// Globals that alone hold an object go newest first (freeing what only they
// hold), then every other object in creation order.
class Dd { function __construct(public $n) {} function __destruct() { echo "d{$this->n}\n"; } }
class Ee { public $x; function __destruct() { echo "e\n"; } }
register_shutdown_function(function ($t) {
    echo "sd $t\n";
    register_shutdown_function(fn() => print("nested\n"));
}, "x");
$a = new Dd(1);
$b = new Dd(2);
$c = [new Dd(3)];
$e = new Dd(4);
$f = $e;
$o = new Ee;
$o->x = new Dd(5);
$k = function () use ($a) { return $a; };
for ($i = 0; $i < 3; $i++) { $j = $i; }
$explicit = new Dd(6);
$explicit->__destruct();
ob_start();
echo "end $j\n";
#==#
// ── destructors still run after an uncaught exception and after exit ──
class Dx { function __destruct() { echo "dx\n"; } }
register_shutdown_function(fn() => print("sd\n"));
$d = new Dx;
throw new Exception("boom");
#==#
class Dx { function __destruct() { echo "dx\n"; } }
$d = new Dx;
exit(4);
#==#
// ── E_USER_ERROR runs shutdown functions but no destructor ──
class Dx { function __destruct() { echo "dx\n"; } }
register_shutdown_function(fn() => print("sd\n"));
$d = new Dx;
trigger_error("x", E_USER_ERROR);
#==#
// ── an exception escaping a shutdown function or destructor is uncaught ──
register_shutdown_function(function () { throw new Exception("in sd"); });
register_shutdown_function(fn() => print("never\n"));
echo "a\n";
#==#
class Dt { function __destruct() { throw new Exception("dt"); } }
class Dy { function __destruct() { echo "dy\n"; } }
$t = new Dt;
$y = new Dy;
echo "a\n";
#==#
// ── suspended generators in globals are destroyed newest first ──
function gfin($n) { try { yield 1; } finally { echo "F$n\n"; } }
$ga = gfin("a"); $ga->current();
$gb = gfin("b"); $gb->current();
echo "end\n";
#==#
// ── set_exception_handler is a stack; under -r it is never called ──
var_dump(set_exception_handler("strlen"), set_exception_handler(null),
         restore_exception_handler(), set_exception_handler("trim"));
set_exception_handler(function ($e) { echo "never\n"; });
throw new Exception("direct");
#==#
// ── set_error_handler takes the diagnostic instead of the display ──
set_error_handler(function ($no, $str, $file, $line) {
    echo "H($no) $str @ $file:$line er=", error_reporting(), "\n";
    return true;
});
$a = [1];
echo $a[9];
echo "cont\n";
echo @$u;
echo "Array: " . [1], "\n";
$x = "5 apples" + 1;
trigger_error("user dep", E_USER_DEPRECATED);
$s = "abc";
echo $s[10], "|\n";
var_dump(error_get_last());
#==#
// ── false falls through to the default display; levels mask what it takes ──
set_error_handler(fn($no, $str) => false);
$a = [];
echo $a[1];
var_dump(error_get_last()["message"]);
set_error_handler(function ($no, $str) { echo "W:$str\n"; }, E_WARNING);
echo $u;
trigger_error("a notice");
var_dump(restore_error_handler(), restore_error_handler());
echo $v;
#==#
// ── the handler stack, and a handler's own diagnostics use the default ──
var_dump(set_error_handler("strlen"), set_error_handler(null),
         restore_error_handler(), set_error_handler("trim", E_WARNING));
restore_error_handler(); restore_error_handler();
set_error_handler(function ($no, $str) { echo "H:$str\n"; echo $inner; restore_error_handler(); });
echo $u1;
echo $u2;
#==#
// ── ErrorException from a handler is catchable where the warning arose ──
set_error_handler(function ($no, $str, $file, $line) {
    throw new ErrorException($str, 0, $no, $file, $line);
});
try {
    $a = [];
    $b = $a["k"];
    echo "not reached\n";
} catch (ErrorException $e) {
    echo get_class($e), " ", $e->getMessage(), " sev=", $e->getSeverity(), " line=", $e->getLine(), "\n";
}
$e = new ErrorException("m", 1, E_WARNING, "f.php", 3);
var_dump($e->getSeverity(), $e->getFile(), $e->getLine(), $e->getCode(), $e instanceof Exception);
echo $undefined_after;
#==#
// ── E_USER_ERROR goes to a handler that takes it; false lets the fatal through ──
set_error_handler(function ($no, $str) { echo "H($no):$str\n"; return $no !== E_USER_ERROR || $str !== "fatal"; });
trigger_error("handled", E_USER_ERROR);
echo "after\n";
trigger_error("fatal", E_USER_ERROR);
echo "never\n";
#==#
// ── the unplaced-name refusal's trace shows the head parameters converted ──
function show_unplaced() { return sprintf(1, 1, "a", [3], nosuch: 2); }
show_unplaced();
#==#
printf(1.5, 2, x: 1);
#==#
// ── float conversions: non-finite values print bare; -0.0 and + on e/g ──
foreach (["%f", "%F", "%e", "%E", "%g", "%G", "%10.1f", "%-10f|", "%+f", "%05f", "%.0f",
          "%+.1e", "%+g", "%+e", "%010.2e", "%-8g|", "%.3g", "%+.2f"] as $f) {
    echo $f, " => [", sprintf($f, INF), "] [", sprintf($f, -INF), "] [", sprintf($f, NAN),
         "] [", sprintf($f, -0.0), "] [", sprintf($f, -0.0001), "] [", sprintf($f, 1234.5678), "]\n";
}
#==#
// ── setlocale: portable answers only (the LC_* values and names are the platform's) ──
var_dump(setlocale(LC_ALL, "C"), setlocale(LC_ALL, ["xx_XX", "C"]),
         setlocale(LC_ALL, "xx_XX", "yy_YY"), setlocale(LC_ALL, [], "C"),
         setlocale(LC_NUMERIC, "0"));
var_dump(setlocale(LC_ALL, str_repeat("a", 300)));
printf("%.2f %g\n", 1.5, 2.5);
try { setlocale("x", "C"); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
#==#
// ── hex2bin warns before answering false ──
var_dump(hex2bin("aaa"), hex2bin("zz"), hex2bin("a"), hex2bin(""), hex2bin("4142"));
#==#
// ── `final` on a private method is a compile-time warning (not on __construct) ──
echo "first\n";
class FinPriv {
    final
    private function f() {}
    final private function __construct() {}
    private final static function g() {}
}
trait FinTrait { final private function t() {} }
#==#
// ── iterator_* refuse what is not Traversable (or an array, where allowed) ──
foreach ([fn() => iterator_to_array(1), fn() => iterator_to_array(new stdClass),
          fn() => iterator_count("x"), fn() => iterator_apply([1], fn() => true)] as $f) {
    try { $f(); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
}
function gen_it() { yield 1; yield 2; }
var_dump(iterator_to_array(gen_it()), iterator_count([1, 2]), iterator_count(new ArrayIterator([1, 2, 3])));
#==#
// ── a NAMED argument to a library function's by-reference parameter is written back ──
preg_match(subject: "ab", pattern: "/(b)/", matches: $m, flags: PREG_OFFSET_CAPTURE);
var_dump($m);
$r = preg_replace("/a/", "b", "aaa", count: $c);
var_dump($r, $c);
str_replace("a", "b", "aaa", count: $n);
parse_str("x=1&y=2", result: $out);
similar_text("World", "Word", percent: $p);
var_dump($n, $out, $p);
#==#
// ── a syntax error quotes a long token's first 30 bytes and `...` ──
if ($a 'a long single-quoted string that goes past thirty bytes') {}
#==#
// ── an unterminated single-quoted string is quoted from the rest of the file ──
echo 'never closed \' still open
and the next line is cut off
#==#
// ── an unterminated double-quoted string: what was expected depends on its content ──
echo "plain text, no closing quote
#==#
echo "with $interpolation and no closing quote
#==#
// ── a nested destructuring pattern spelled differently from its parent is a compile error ──
echo "never printed\n";
foreach ([[1, [2]]] as [$a, list($b)]) {}
#==#
// ── a destructuring pattern that binds nothing is an empty list ──
echo "never printed\n";
[$a, [, ]] = [1, [2]];
#==#
// ── a target that is not a place to store into ──
echo "never printed\n";
[$a, $o->m()] = [1, 2];
#==#
// ── a gap in an array literal that is not a destructuring target ──
echo "never printed\n";
$a = [1, , 2];
#==#
// ── gaps, keys and nesting in a valid pattern still bind ──
[, $b, [, $c]] = [1, 2, [3, 4]];
list(, list(, $d)) = [5, [6, 7]];
['x' => $x, 'y' => ['z' => $z]] = ['x' => 8, 'y' => ['z' => 9]];
var_dump($b, $c, $d, $x, $z);
#==#
// ── a default ahead of a required parameter is dropped, and deprecated ──
function pd_f(int $a = null, $b = 1, $c) { var_dump($a); }
class PdK { function m(?int $a = null, $b) {} }
$pd = fn(string $s = null) => $s ?? "dflt";
pd_f(null, 2, 3);
echo $pd(), "\n";
try { pd_f(5, c: 1); } catch (ArgumentCountError $e) { echo $e->getMessage(), "\n"; }
#==#
// ── a named argument past a hole: the trace shows NULL in the empty slots ──
function hole_f($a, $b, $c) {}
hole_f(1, c: 3);
#==#
// ── a continue that lands on a switch warns once at compile time ──
function sw_f() { foreach ([1, 2] as $x) { switch ($x) { case 1: continue; default: echo $x, "\n"; } } }
sw_f();
#==#
// ── a break level no loop can reach ──
echo "never printed\n";
foreach ([1] as $x) { while (1) { try { break 3; } finally {} } }
#==#
// ── reading $this with no object bound is an Error, not an undefined variable ──
function no_this() { return $this; }
try { no_this(); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
$unbound = function () { return isset($this) ? "bound" : "unbound"; };
echo $unbound(), "\n";
class HasThis { public $v = 4; function m() { $f = fn() => $this->v; return $f() * 2; } }
echo (new HasThis)->m(), "\n";
#==#
// ── a void function that returns a value is a compile error ──
echo "never printed\n";
function void_f(): void { if (true) { return null; } }
#==#
// ── a backed enum case needs a value ──
echo "never printed\n";
enum NoValue: string { case A = "a"; case B; }
#==#
// ── a use clause naming a parameter ──
echo "never printed\n";
$f = function ($a) use ($a) { return $a; };
#==#
// ── strtotime is timelib's scanner: formats, zones, weekday and "of" relatives ──
$base = 1709254923; // 2024-03-01 01:02:03 UTC, a Friday
foreach (["15 January 2024", "01/15/2024", "Jan 15th 2024 5:30pm", "2024-W03-2", "Sat, 30 Apr 2016 17:52:13 GMT",
          "2024-03-01T10:00:00+05:30", "10:00 EST", "next monday", "last friday of this month",
          "first day of next month", "+5 weekdays", "3 days ago", "tomorrow noon", "2024-01-31 +1 month",
          "@1709251200.5", "   ", "foo", "10:00 10:00"] as $s) {
    $r = strtotime($s, $base);
    echo str_pad($s, 34), $r === false ? "false" : gmdate("Y-m-d H:i:s", $r), "\n";
}
#==#
// ── DateTime: calendar arithmetic, zones, intervals and the shown properties ──
$d = new DateTime("2024-01-31 10:00:00.25");
$month = new DateInterval("P1M");
$d->add($month);
var_dump($d);
$from = new DateTime("2024-01-01");
$to = new DateTimeImmutable("2024-03-15 10:00 +02:00");
$i = $from->diff($to);
var_dump($i);
echo $i->format("%R%a days, %y-%m-%d %H:%I:%S.%F"), "\n";
$z = new DateTimeImmutable("2024-06-01 12:00:00", new DateTimeZone("+05:30"));
echo $z->format(DATE_RFC2822), " ", $z->setTimezone(new DateTimeZone("EST"))->format("c T e I"), "\n";
echo json_encode(new DateTime("2000-01-01 00:00 PST")), "\n";
var_dump(new DateTime("2020-02-29") < new DateTimeImmutable("2020-03-01"), date_parse("next monday 10am")["relative"]);
#==#
// ── DateTime refusals: the reference's exception classes and messages ──
foreach ([fn() => new DateTime("31/12/2024"), fn() => new DateInterval("P1X"),
          fn() => (new DateTimeImmutable("2020-01-01"))->modify("noonish"),
          fn() => DateInterval::createFromDateString("2020-01-01"),
          fn() => new DateTimeZone("+99:00")] as $f) {
    try { $f(); } catch (Exception $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
}
var_dump(date_create("bogus"), date_get_last_errors());
new DateTime("2024-02-31 25:00");
echo DateTime::getLastErrors()["warnings"][16] ?? "none", "\n";
#==#
// ── goto: backward loops, jumps out of nested loops, function-scoped labels ──
function collatz($n) { $steps = 0; again: if ($n == 1) goto done; $n = $n % 2 ? 3 * $n + 1 : intdiv($n, 2); $steps++; goto again; done: return $steps; }
echo collatz(27), "\n";
foreach ([1, 2] as $a) { foreach ([3, 4] as $b) { if ($b == 4) goto out; echo $a, $b, " "; } }
out:
echo "out\n";
#==#
// ── pack / unpack (bytes below 0x80) ──
echo bin2hex(pack("nvNVc*", 0x1234, 0x1234, 0x01020304, 0x01020304, 65, 66)), "\n";
print_r(unpack("nbig/vlittle/C2c", "\x12\x34\x34\x12AB"));
echo json_encode(unpack("A5a/Z*z", "ab   cd\0ef")), " ", json_encode(pack("a4A4Z4", "x", "y", "zzzz")), "\n";
echo pack("H*", "50485021"), " ", json_encode(unpack("H*", "PHP")), "\n";
var_dump(unpack("N", "ab"));
try { unpack("y", "a"); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
#==#
// ── declared types: classes, unions, pseudo-types, typed properties ──
interface Shape {}
class Sq implements Shape {}
class TB { public int $n = 0; public ?TB $next = null; public static float $rate = 1.0;
    public static function make(): static { return new TB; }
    public function __construct(public array $tags = []) {} }
class TD extends TB {}
function shape(Shape $s): string { return get_class($s); }
function num(int|float|bool $n) { var_dump($n); }
echo shape(new Sq), "\n"; num("7"); num("1.5"); num("x1");
$t = new TB; $t->n = "12"; TB::$rate = 2; var_dump($t->n, TB::$rate);
foreach ([fn() => shape(new TD), fn() => TD::make(), fn() => $t->next = new Sq, fn() => $t->tags = "x"] as $f) {
    try { $f(); } catch (TypeError $e) { echo preg_replace('/, called in.*/', '', $e->getMessage()), "\n"; }
}
$it = new ArrayIterator([5, 6]); foreach ($it as $k => $v) echo "$k:$v ";
try { $it->seek(4); } catch (OutOfBoundsException $e) { echo $e->getMessage(), "\n"; }
#==#
// Exception::__toString renders the whole `previous` chain, innermost first,
// each with its own trace; an empty message drops the `: `; a user
// TypeError's ", called in" message gains " and defined".
function thrower() { throw new DomainException("in fn"); }
class TS { function m($a) { thrower(); } }
try { (new TS)->m([1, 2]); } catch (Exception $e) { echo $e, "\n---\n"; var_dump((string)$e === $e->__toString()); }
echo new LogicException("outer", 1, new InvalidArgumentException("inner")), "\n";
echo new Error("", 0, new TypeError("t")), "\n";
function typed(int $x) {}
try { typed("x"); } catch (TypeError $e) { echo $e, "\n"; }
echo new ErrorException("ee", 3, E_WARNING, "x.php", 9), "\n";
#==#
// An uncaught exception is reported through its own __toString, previous
// chain included.
throw new LogicException("outer", 1, new RuntimeException("inner"));
#==#
// A user __toString override is what the uncaught fatal prints.
class MyEx extends Exception { public function __toString(): string { return "custom!"; } }
throw new MyEx("m");
#==#
// An exception thrown BY __toString while reporting is what goes uncaught,
// from an [internal function] frame.
class BadEx extends Exception { public function __toString(): string { throw new Exception("boom"); } }
throw new BadEx("m");
#==#
// __debugInfo replaces what var_dump and print_r show; "\0Class\0p" and "\0*\0p" keys
// read as private and protected, an int key prints bare, null is deprecated.
class D { public $x = 1; protected $hid = 0; function __debugInfo() { return ["\0D\0flags" => 1, "\0*\0p" => 2, "z" => 3, 7 => [new E]]; } }
class E { private $q = 5; function __debugInfo() { return ['shown' => $this->q * 2]; } }
var_dump(new D); print_r(new D); echo "\n"; print_r([new D, 1], false); echo print_r(new E, true), "\n";
var_export(new D); echo "\n"; var_dump((array)new D, get_object_vars(new E));
class N { function __debugInfo() { return null; } }
print_r(new N); echo "
";
#==#
// serialize() writes what __serialize() answers; unserialize() hands __unserialize()
// the data once the payload has parsed. A non-array answer is a TypeError.
class U { function __serialize() { return 5; } }
try { serialize(new U); } catch (Throwable $e) { echo get_class($e), $e->getMessage(), "\n", $e->getTraceAsString(), "\n"; }
class V { public $a = 1; function __serialize(): array { return ["x" => 1, 3 => [2], "\0*\0p" => 1]; } function __unserialize(array $d): void { echo "unser:"; var_dump($d); $this->a = $d["x"] + 10; } }
echo $s = serialize([new V, new V]), "\n"; foreach (unserialize($s) as $o) echo $o->a, "\n";
$st = new SplStack; $st->push(1); $st->push([2]); $st->dyn = "d"; echo $s = serialize($st), "\n"; $u = unserialize($s); print_r($u); foreach ($u as $v) echo json_encode($v); echo "\n";
$h = new SplMinHeap; $h->insert(3); $h->insert(1); echo $s = serialize($h), "\n"; $u = unserialize($s); echo $u->extract(), $u->count(), "\n";
$pq = new SplPriorityQueue; $pq->insert("a", 2); $pq->insert("b", 9); echo $s = serialize($pq), "\n"; echo unserialize($s)->extract(), "\n";
$f = SplFixedArray::fromArray([1, 2]); echo $s = serialize($f), "\n"; var_dump(unserialize($s)->toArray());
#==#
// SplDoublyLinkedList/SplQueue/SplStack, SplHeap/SplPriorityQueue and SplFixedArray
// follow ext/spl: iterator modes, heap sift order for ties, corruption, messages.
$pq = new SplPriorityQueue;
foreach (["a" => 1, "b" => 3, "c" => 3, "d" => 2, "e" => 3, "f" => 1, "g" => 2] as $d => $p) $pq->insert($d, $p);
$pq->setExtractFlags(SplPriorityQueue::EXTR_BOTH); print_r($pq->top()); $pq->setExtractFlags(SplPriorityQueue::EXTR_DATA);
foreach ($pq as $k => $v) echo "$k:$v "; echo count($pq), "\n";
class RevHeap extends SplHeap { protected function compare($a, $b): int { return strlen($b) - strlen($a); } }
$h = new RevHeap; foreach (["ccc", "a", "bb", "dddd", "e", "ff"] as $x) $h->insert($x); while ($h->valid()) { echo $h->key(), "=", $h->current(), " "; $h->next(); } echo "\n";
class Bad extends SplMinHeap { public $boom = false; protected function compare($a, $b): int { if ($this->boom) throw new LogicException("cmp"); return parent::compare($a, $b); } }
$b = new Bad; $b->insert(1); $b->insert(2); $b->boom = true;
try { $b->insert(0); } catch (LogicException $e) { echo "caught ", $e->getMessage(), " ", var_export($b->isCorrupted(), true), count($b), "\n"; }
try { $b->top(); } catch (RuntimeException $e) { echo $e->getMessage(), "\n"; }
$b->recoverFromCorruption(); $b->boom = false; echo $b->extract(), "\n";
try { (new SplMinHeap)->extract(); } catch (RuntimeException $e) { echo $e->getMessage(), "\n"; }
try { new SplHeap; } catch (Error $e) { echo $e->getMessage(), "\n"; }
try { $pq->setExtractFlags(0); } catch (RuntimeException $e) { echo $e->getMessage(), "\n"; }
$l = new SplDoublyLinkedList; foreach ([1, 2, 3, 4] as $x) $l->push($x);
$l->setIteratorMode(SplDoublyLinkedList::IT_MODE_LIFO | SplDoublyLinkedList::IT_MODE_DELETE);
foreach ($l as $k => $v) echo "$k=$v "; echo count($l), "\n";
$l = new SplDoublyLinkedList; foreach ([1, 2, 3] as $x) $l->push($x); $l->add(1, "X"); $l->unshift(0); $l->offsetUnset(2); print_r($l); echo $l->getIteratorMode(), "\n";
$l->rewind(); $l->next(); echo $l->current(), $l->key(); $l->prev(); echo $l->current(), $l->key(), "\n";
$q = new SplQueue; $q[] = 1; $q[] = 2; echo $q->dequeue(), $q[0], "\n"; var_dump($q->isEmpty(), $q->toArray ?? null);
try { $q->setIteratorMode(SplDoublyLinkedList::IT_MODE_LIFO); } catch (RuntimeException $e) { echo $e->getMessage(), "\n"; }
$s = new SplStack; $s[] = 1; $s[] = 2; echo $s[0], $s->top(), $s->bottom(), "\n"; $c = clone $s; $c->push(9); echo count($s), count($c), "\n";
var_dump($s instanceof Iterator, $s instanceof ArrayAccess, $s instanceof Countable, $h instanceof Iterator);
$f = new SplFixedArray(3); $f[0] = "a"; echo count($f), isset($f[1]) ? "y" : "n", isset($f[0]) ? "y" : "n", isset($f[7]) ? "y" : "n", "\n";
$f->setSize(1); print_r($f->toArray()); $f->setSize(2); print_r($f); echo json_encode($f), "\n";
print_r(SplFixedArray::fromArray([3 => "x", 1 => "y"])->toArray()); print_r(SplFixedArray::fromArray(["p" => 1, "q" => 2], false)->toArray());
try { SplFixedArray::fromArray(["p" => 1]); } catch (InvalidArgumentException $e) { echo $e->getMessage(), "\n"; }
foreach (SplFixedArray::fromArray([7, 8]) as $k => $v) echo "$k$v"; echo "\n";
#==#
// An exception thrown inside a prelude (internal) method carries the caller's line.
$it = new ArrayIterator([1]);

try { $it->seek(5); } catch (Exception $e) { echo $e->getLine(), " ", $e->getFile(), "\n", $e->getTraceAsString(), "\n"; }
function f() { $it = new ArrayIterator([1]); $it->seek(9); }
f();
#==#
// The prelude's closures keep their own names when the program declares
// closures of its own, and a prelude method's helper frames stay out of a
// trace: the user method it calls back into was entered from internal code.
class H extends SplMinHeap { protected function compare($a, $b): int { throw new Exception("x"); } }
$h = new H; $h->insert(1);
try { $h->insert(2); } catch (Exception $e) { echo $e->getTraceAsString(), "\n"; }
$f = function ($x, $y) {};
#==#
// A by-value foreach over an Iterator runs its methods interleaved with the
// body, current() before key(); an IteratorAggregate hands over its iterator
// (through nested aggregates), and one answering a non-Traversable is refused.
class T implements Iterator { private $i = 0; function current(): mixed { echo "cur "; return $this->i; } function key(): mixed { echo "key "; return $this->i; } function next(): void { echo "next "; $this->i++; } function rewind(): void { echo "rew "; $this->i = 0; } function valid(): bool { echo "valid "; return $this->i < 2; } }
foreach (new T as $k => $v) echo "[$k$v] "; echo "\n";
foreach (new T as $v) { echo "<$v> "; if ($v) break; } echo "\n";
class A implements IteratorAggregate { #[\ReturnTypeWillChange] function getIterator() { return [1]; } }
try { foreach (new A as $v) echo $v; } catch (Exception $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
class B implements IteratorAggregate { function getIterator(): Iterator { return new ArrayIterator(["k" => "v"]); } }
class C implements IteratorAggregate { function getIterator(): Traversable { return new B; } }
foreach (new C as $k => $v) echo "$k=$v\n";
function g() { yield 1 => "a"; yield 1 => "b"; } foreach (g() as $k => $k) echo $k; echo "\n";
$c = new CachingIterator(new ArrayIterator(['a', 'b', 'c'])); foreach ($c as $v) echo $v, $c->hasNext() ? "," : "\n";
$n = 0; foreach (new InfiniteIterator(new ArrayIterator([1, 2])) as $v) { echo $v; if (++$n > 4) break; } echo "\n";
#==#
// The SPL iterators of ext/spl/spl_iterators.c: the dual iterators.
$l = new LimitIterator(new ArrayIterator([1, 2, 3, 4, 5]), 1, 2); foreach ($l as $k => $v) echo "$k=$v "; echo $l->getPosition(), "\n";
echo $l->seek(2), "\n";
try { $l->seek(0); } catch (OutOfBoundsException $e) { echo $e->getMessage(), "\n"; }
try { $l->seek(5); } catch (OutOfBoundsException $e) { echo $e->getMessage(), "\n"; }
try { new LimitIterator(new ArrayIterator([]), -1); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
echo $l->count(), "\n";
function g4() { yield 1; yield 2; yield 3; yield 4; } foreach (new LimitIterator(g4(), 1, 2) as $k => $v) echo "$k=$v "; echo "\n";
$c = new CachingIterator(new ArrayIterator(['x' => 1, 'y' => [2], 'z' => 3]), CachingIterator::FULL_CACHE);
foreach ($c as $k => $v) { echo $k, ":", (string) $c, $c->hasNext() ? "+" : ".", " "; } echo "\n";
print_r($c->getCache()); echo count($c), $c['x'], "\n"; var_dump(isset($c['q'])); var_dump($c['q']);
$c = new CachingIterator(new ArrayIterator([1]), CachingIterator::TOSTRING_USE_KEY); foreach ($c as $v) echo (string) $c, "\n";
try { $c->getCache(); } catch (BadMethodCallException $e) { echo $e->getMessage(), "\n"; }
try { new CachingIterator(new ArrayIterator([]), 3); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
$c = new CachingIterator(new ArrayIterator([]), 0); try { echo (string) $c; } catch (BadMethodCallException $e) { echo $e->getMessage(), "\n"; } echo $c->getFlags(), "\n";
try { (new CachingIterator(new ArrayIterator([])))->setFlags(0); } catch (InvalidArgumentException $e) { echo $e->getMessage(), "\n"; }
$a = new AppendIterator; $a->append(new ArrayIterator(['a' => 1, 'b' => 2])); $a->append(new ArrayIterator([])); $a->append(new ArrayIterator(['c' => 3]));
foreach ($a as $k => $v) echo "$k=$v@", $a->getIteratorIndex(), " "; echo "\n";
var_dump($a->getIteratorIndex(), $a->valid(), $a->current()); echo get_class($a->getArrayIterator()), count($a->getArrayIterator()), "\n";
$f = new CallbackFilterIterator(new ArrayIterator(['a' => 1, 'b' => 2, 'c' => 3]), function ($v, $k, $it) { echo "[$k]"; return $v != 2; });
foreach ($f as $k => $v) echo "$k=$v "; echo get_class($f->getInnerIterator()), "\n";
class OddFilter extends FilterIterator { public function accept(): bool { return $this->current() % 2 == 1; } } print_r(iterator_to_array(new OddFilter(new ArrayIterator(range(1, 7)))));
$n = new NoRewindIterator(new ArrayIterator([1, 2, 3])); $n->getInnerIterator()->next(); foreach ($n as $v) echo $v; foreach ($n as $v) echo $v; echo "\n";
$e = new EmptyIterator; foreach ($e as $v) echo "never"; var_dump($e->valid()); try { $e->current(); } catch (BadMethodCallException $x) { echo $x->getMessage(), "\n"; }
$ii = new IteratorIterator(new ArrayObject([1, 2])); foreach ($ii as $k => $v) echo "$k$v"; echo get_class($ii->getInnerIterator()), "\n";
try { $ii->__construct(new ArrayIterator([])); } catch (BadMethodCallException $e) { echo $e->getMessage(), "\n"; }
class Sub extends LimitIterator { function __construct() {} } try { (new Sub)->rewind(); } catch (Error $e) { echo $e->getMessage(), "\n"; }
print_r(new LimitIterator(new ArrayIterator([1])));
echo iterator_count(new LimitIterator(new InfiniteIterator(new ArrayIterator([1, 2, 3])), 0, 7)), "\n";
#==#
// RegexIterator's five modes and its flags.
$r = new RegexIterator(new ArrayIterator(['a1', 'b', 'c22', 'd']), '/\d/', RegexIterator::MATCH, RegexIterator::INVERT_MATCH); print_r(iterator_to_array($r));
$r = new RegexIterator(new ArrayIterator(['a1' => 'x', 'b' => 'y']), '/\d/', RegexIterator::MATCH, RegexIterator::USE_KEY); print_r(iterator_to_array($r));
$r = new RegexIterator(new ArrayIterator(['ab12', 'cd']), '/(\w)(\d)/', RegexIterator::GET_MATCH); print_r(iterator_to_array($r));
echo $r->getRegex(), $r->getMode(), $r->getFlags(), $r->getPregFlags(), "\n";
try { new RegexIterator(new ArrayIterator([]), "abc"); } catch (Exception $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { new RegexIterator(new ArrayIterator([]), "/(/"); } catch (Exception $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { new RegexIterator(new ArrayIterator([]), "/x/", 9); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
print_r(iterator_to_array(new RegexIterator(new ArrayIterator(["a1b2", "cc", "d3"]), "/\d/", RegexIterator::ALL_MATCHES)));
print_r(iterator_to_array(new RegexIterator(new ArrayIterator(["a1b2", "cc"]), "/\d/", RegexIterator::SPLIT)));
$r = new RegexIterator(new ArrayIterator(["k1" => "a1", "zz" => "b"]), "/\d/", RegexIterator::REPLACE); $r->replacement = "#"; print_r(iterator_to_array($r)); print_r($r);
#==#
// RecursiveIteratorIterator's three orders, its depth limit and hooks, and
// RecursiveTreeIterator, ParentIterator and the recursive filters over it.
$it = new RecursiveIteratorIterator(new RecursiveArrayIterator([1, [2, [3, 4]], 5, []])); foreach ($it as $k => $v) echo $it->getDepth(), "$k=$v "; echo "\n";
foreach (new RecursiveIteratorIterator(new RecursiveArrayIterator(['a' => 1, 'b' => ['c' => 2, 'd' => ['e' => 3]]]), RecursiveIteratorIterator::CHILD_FIRST) as $k => $v) echo $k, is_array($v) ? "[]" : "=$v", " "; echo "\n";
$it = new RecursiveIteratorIterator(new RecursiveArrayIterator([1, [2, [3, 4]], 5]), RecursiveIteratorIterator::SELF_FIRST); $it->setMaxDepth(1);
foreach ($it as $k => $v) echo $it->getDepth(), ":", is_array($v) ? "A" : $v, " "; echo "\n";
var_dump($it->getMaxDepth()); try { $it->setMaxDepth(-2); } catch (ValueError $e) { echo $e->getMessage(), "\n"; } $it->setMaxDepth(); var_dump($it->getMaxDepth());
print_r(iterator_to_array(new RecursiveIteratorIterator(new RecursiveArrayIterator([[1, 2], [3]])), false));
class Hooked extends RecursiveIteratorIterator { function beginIteration(): void { echo "<begin>"; } function endIteration(): void { echo "<end>"; } function beginChildren(): void { echo "<down>"; } function endChildren(): void { echo "<up>"; } function nextElement(): void { echo "<el>"; } function callHasChildren(): bool { echo "?"; return parent::callHasChildren(); } }
foreach (new Hooked(new RecursiveArrayIterator([1, [2, 3], 4]), RecursiveIteratorIterator::SELF_FIRST) as $k => $v) echo is_array($v) ? "A" : $v; echo "\n";
$t = new RecursiveTreeIterator(new RecursiveArrayIterator(['a' => 1, 'b' => ['c' => 2, 'd' => ['e' => 3]], 'f' => 4])); foreach ($t as $k => $v) echo "[$k] $v\n";
$t = new RecursiveTreeIterator(new RecursiveArrayIterator(['x' => ['y' => 1]]), 0); foreach ($t as $k => $v) echo "$k => $v\n";
$t->setPrefixPart(RecursiveTreeIterator::PREFIX_LEFT, ">"); $t->setPostfix("<"); foreach ($t as $v) echo $v, "|", $t->getEntry(), "|", $t->getPrefix(), "|", $t->getPostfix(), "\n";
$p = new ParentIterator(new RecursiveArrayIterator([1, 'a' => [2, 'b' => [3]], 4])); foreach (new RecursiveIteratorIterator($p, RecursiveIteratorIterator::SELF_FIRST) as $k => $v) echo $k, " "; echo "\n";
$f = new RecursiveCallbackFilterIterator(new RecursiveArrayIterator([1, [2, 30], 40]), fn($v, $k, $it) => $it->hasChildren() || $v < 10); foreach (new RecursiveIteratorIterator($f) as $v) echo $v, " "; echo "\n";
$a = new ArrayIterator([3, 4], ArrayIterator::ARRAY_AS_PROPS); echo $a->getFlags(), "\n";
$r = new RecursiveArrayIterator([[1]], RecursiveArrayIterator::CHILD_ARRAYS_ONLY); var_dump($r->hasChildren(), $r->getChildren()->getFlags(), $r->getChildren() instanceof RecursiveArrayIterator);
#==#
// Traces through the SPL iterators: a method the reference calls from C is
// entered from [internal function], an iterator it walks natively has no
// frame, and a forwarded call that finds nothing names the outer class.
$c = new CallbackFilterIterator(new ArrayIterator([1, 2]), function ($v) { throw new Exception("x"); });
try { foreach ($c as $v) {} } catch (Exception $e) { echo $e->getTraceAsString(), "\n"; }
class F extends FilterIterator { function accept(): bool { throw new Exception("y"); } }
try { iterator_to_array(new F(new ArrayIterator([1]))); } catch (Exception $e) { echo $e->getTraceAsString(), "\n"; }
$l = new LimitIterator(new ArrayIterator([1]));
try { $l->seek(4); } catch (Exception $e) { echo $e->getLine(), $e->getTraceAsString(), "\n"; }
class R implements RecursiveIterator { private $i = 0; function current(): mixed { return $this->i; } function key(): mixed { return $this->i; } function next(): void { $this->i++; } function rewind(): void { $this->i = 0; } function valid(): bool { return $this->i < 2; } function hasChildren(): bool { throw new LogicException("hc"); } function getChildren(): ?RecursiveIterator { return null; } }
foreach (new RecursiveIteratorIterator(new R, 0, RecursiveIteratorIterator::CATCH_GET_CHILD) as $v) echo $v; echo "\n";
try { foreach (new RecursiveIteratorIterator(new R) as $v) echo $v; } catch (LogicException $e) { echo $e->getTraceAsString(), "\n"; }
$l->nope();
#==#
// #[\Deprecated] on a function or method: the deprecation names the declaring
// class, carries `since` and `message`, is E_USER_DEPRECATED, and is raised
// before the callee binds its arguments — a throwing handler stops the call.
class A { #[\Deprecated(since: "8.5", message: "use x instead")] function f() {} }
(new A)->f();
#[\Deprecated] function g() { echo "ran\n"; } g();
#[Deprecated("m")] function h($x) {} h(1);
class B extends A {} (new B)->f();
set_error_handler(function ($n, $s) { echo "[$n] $s\n"; return true; }); g();
set_error_handler(function ($n, $s) { throw new Exception($s); });
#[\Deprecated] function t(int $x) { echo "body"; }
try { t("a"); } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
#==#
// SplObjectStorage, a port of ext/spl/spl_observer.c: storage order, the
// iteration pointer and index, seek, info, the hash a subclass supplies, and
// the debug and serialization views.
$s = new SplObjectStorage; $a = new stdClass; $b = new stdClass; $s[$a] = "A"; $s[$b] = "B";
foreach ($s as $i => $o) { echo $i, get_class($o), $s->getInfo(), "\n"; } echo count($s), "\n";
unset($s[$a]); var_dump($s->offsetExists($a), count($s)); var_dump($s);
$s = new SplObjectStorage; $o = new stdClass; $s[$o] = 1; $t = new SplObjectStorage; $t->addAll($s); echo count($t); $t->removeAll($s); echo count($t), "\n";
echo $s->getHash($o) === spl_object_hash($o) ? "y" : "n", "\n"; print_r($s); echo serialize($s), "\n", $s->serialize(), "\n";
$u = unserialize(serialize($s)); echo count($u), get_class($u), "\n";
$s = new SplObjectStorage; $x = [new stdClass, new stdClass, new stdClass]; foreach ($x as $i => $o) $s[$o] = $i * 10;
$s->seek(2); echo $s->key(), $s->getInfo(), "\n"; $s->seek(1); echo $s->key(), $s->getInfo(), "\n";
try { $s->seek(5); } catch (OutOfBoundsException $e) { echo $e->getMessage(), "\n"; }
$s->rewind(); $s->setInfo("new"); echo $s[$x[0]], "\n";
try { $s[new stdClass]; } catch (UnexpectedValueException $e) { echo $e->getMessage(), "\n"; }
try { $s["x"] = 1; } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
var_dump(isset($s[$x[1]]), empty($s[$x[0]]), isset($s[new stdClass]));
$p = new stdClass; $q = new stdClass; $s = new SplObjectStorage; $s[$p] = 1; $s[$q] = 2; $t = new SplObjectStorage; $t[$q] = 9;
echo $s->removeAllExcept($t), $s->count(COUNT_RECURSIVE), "\n";
$s->rewind(); try { $s->next(); $s->current(); } catch (RuntimeException $e) { echo $e->getMessage(), "\n"; }
$c = clone $s; $c[$p] = 5; echo count($s), count($c), "\n";
class HS extends SplObjectStorage { public function getHash($o): string { return get_class($o); } }
$h = new HS; $h[new stdClass] = 1; $h[new stdClass] = 2; echo count($h), $h[new stdClass], "\n";
