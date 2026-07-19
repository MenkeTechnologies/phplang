//! Language corpus for offline documentation generation.
//!
//! A single `(name, chapter, one-line doc, runnable example)` table describing
//! the keywords, language constructs, casts, and library functions the current
//! phplang build actually recognizes. Every entry mirrors real code:
//!   * "Keyword" / "Construct" → the reserved words the parser handles
//!     (`src/parser.rs`, `src/lexer.rs`).
//!   * "Cast" → the `(type)` casts `cast_fn` desugars (`src/parser.rs`).
//!   * "String" / "Array" / "Math" / "Type" / "Output" → real `call_library`
//!     match arms in `src/builtins.rs`.
//!
//! `gen-docs` (`src/bin/gen_docs.rs`) consumes `corpus()` to render
//! `docs/reference.html`, so the reference manual never documents a name the
//! runtime does not implement.

/// The keyword / construct / builtin corpus: (name, chapter, one-line doc,
/// runnable PHP example ending in a `// =>` result comment).
const CORPUS: &[(&str, &str, &str, &str)] = &[
    // ── Keyword (control flow + literals) ──
    (
        "if",
        "Keyword",
        "conditional branch; runs its block when the condition is truthy",
        "if (1 < 2) echo \"y\";   // => y",
    ),
    (
        "elseif",
        "Keyword",
        "additional condition branch tested when the preceding ones fail",
        "if (0) echo \"a\"; elseif (1) echo \"b\";   // => b",
    ),
    (
        "else",
        "Keyword",
        "fallback branch taken when every preceding condition is falsey",
        "if (0) echo \"a\"; else echo \"b\";   // => b",
    ),
    (
        "while",
        "Keyword",
        "loop while the condition stays truthy (tested before each pass)",
        "$i = 0; while ($i < 3) $i++; echo $i;   // => 3",
    ),
    (
        "do",
        "Keyword",
        "do/while loop; the body always runs at least once",
        "$i = 5; do $i++; while ($i < 3); echo $i;   // => 6",
    ),
    (
        "for",
        "Keyword",
        "C-style loop with init, condition, and step clauses",
        "$s = 0; for ($i = 1; $i <= 3; $i++) $s += $i; echo $s;   // => 6",
    ),
    (
        "foreach",
        "Keyword",
        "iterate an array as `$value` or `$key => $value`",
        "foreach ([1, 2] as $v) echo $v;   // => 12",
    ),
    (
        "as",
        "Keyword",
        "binds the current element inside a `foreach` header",
        "foreach ([9] as $k => $v) echo $k;   // => 0",
    ),
    (
        "switch",
        "Keyword",
        "multi-way branch; cases compare with loose (`==`) equality",
        "switch (2) { case 2: echo \"two\"; }   // => two",
    ),
    (
        "case",
        "Keyword",
        "a labelled `switch` branch; falls through until a `break`",
        "switch (1) { case 1: echo \"a\"; case 2: echo \"b\"; }   // => ab",
    ),
    (
        "default",
        "Keyword",
        "the `switch`/`match` branch taken when no case matches",
        "switch (9) { default: echo \"d\"; }   // => d",
    ),
    (
        "break",
        "Keyword",
        "exit the nearest enclosing loop or `switch` immediately",
        "foreach ([1, 2, 3] as $v) { if ($v == 2) break; echo $v; }   // => 1",
    ),
    (
        "continue",
        "Keyword",
        "skip to the next iteration of the nearest loop",
        "foreach ([1, 2, 3] as $v) { if ($v == 2) continue; echo $v; }   // => 13",
    ),
    (
        "function",
        "Keyword",
        "define a named function; `return` yields its value (else null)",
        "function sq($x) { return $x * $x; } echo sq(4);   // => 16",
    ),
    (
        "return",
        "Keyword",
        "return a value from the current function",
        "function one() { return 1; } echo one();   // => 1",
    ),
    (
        "match",
        "Keyword",
        "PHP 8 expression; strict (`===`) compare of the subject to each arm",
        "echo match (2) { 1 => \"a\", 2 => \"b\" };   // => b",
    ),
    (
        "array",
        "Keyword",
        "array literal constructor; `array(...)` is the same as `[...]`",
        "echo count(array(1, 2, 3));   // => 3",
    ),
    (
        "and",
        "Keyword",
        "low-precedence logical AND (short-circuits)",
        "echo (true and false) ? \"y\" : \"n\";   // => n",
    ),
    (
        "or",
        "Keyword",
        "low-precedence logical OR (short-circuits)",
        "echo (false or true) ? \"y\" : \"n\";   // => y",
    ),
    (
        "true",
        "Keyword",
        "the boolean true literal",
        "echo true ? \"y\" : \"n\";   // => y",
    ),
    (
        "false",
        "Keyword",
        "the boolean false literal",
        "echo false ? \"y\" : \"n\";   // => n",
    ),
    (
        "null",
        "Keyword",
        "the null literal; the absent / unset value",
        "echo null ?? \"fallback\";   // => fallback",
    ),
    // ── Construct (language constructs, not functions) ──
    (
        "echo",
        "Construct",
        "write one or more values to stdout (no return value)",
        "echo \"hi\", \"!\";   // => hi!",
    ),
    (
        "print",
        "Construct",
        "write a single value to stdout; evaluates to 1",
        "print \"hi\";   // => hi",
    ),
    (
        "isset",
        "Construct",
        "true if every argument is set and not null (quiet on unset)",
        "$a = 1; echo isset($a) ? \"y\" : \"n\";   // => y",
    ),
    (
        "empty",
        "Construct",
        "true if the argument is falsey or unset (quiet on unset)",
        "echo empty(0) ? \"y\" : \"n\";   // => y",
    ),
    // ── Cast (the (type) casts cast_fn desugars) ──
    (
        "(int)",
        "Cast",
        "cast to integer (desugars to intval)",
        "echo (int) \"42px\";   // => 42",
    ),
    (
        "(float)",
        "Cast",
        "cast to float (desugars to floatval); also (double)/(real)",
        "echo (float) \"3.5\";   // => 3.5",
    ),
    (
        "(string)",
        "Cast",
        "cast to string (desugars to strval)",
        "echo (string) 123;   // => 123",
    ),
    (
        "(bool)",
        "Cast",
        "cast to boolean (desugars to boolval); also (boolean)",
        "echo (bool) 0 ? \"y\" : \"n\";   // => n",
    ),
    // ── String (call_library) ──
    (
        "strlen",
        "String",
        "byte length of a string",
        "echo strlen(\"abc\");   // => 3",
    ),
    (
        "strtoupper",
        "String",
        "copy with all letters uppercased",
        "echo strtoupper(\"abc\");   // => ABC",
    ),
    (
        "strtolower",
        "String",
        "copy with all letters lowercased",
        "echo strtolower(\"ABC\");   // => abc",
    ),
    (
        "ucfirst",
        "String",
        "copy with the first character uppercased",
        "echo ucfirst(\"hello\");   // => Hello",
    ),
    (
        "lcfirst",
        "String",
        "copy with the first character lowercased",
        "echo lcfirst(\"Hello\");   // => hello",
    ),
    (
        "ucwords",
        "String",
        "copy with the first letter of each word uppercased",
        "echo ucwords(\"a b\");   // => A B",
    ),
    (
        "trim",
        "String",
        "copy with leading and trailing whitespace removed",
        "echo trim(\"  hi  \");   // => hi",
    ),
    (
        "ltrim",
        "String",
        "copy with leading whitespace removed",
        "echo ltrim(\"  hi\");   // => hi",
    ),
    (
        "rtrim",
        "String",
        "copy with trailing whitespace removed (alias `chop`)",
        "echo rtrim(\"hi  \") . \"!\";   // => hi!",
    ),
    (
        "str_repeat",
        "String",
        "the string repeated n times",
        "echo str_repeat(\"ab\", 3);   // => ababab",
    ),
    (
        "strrev",
        "String",
        "the string with its characters reversed",
        "echo strrev(\"abc\");   // => cba",
    ),
    (
        "wordwrap",
        "String",
        "wrap a string to a column width at word boundaries",
        "echo wordwrap(\"a b c\", 3, \"|\", true);   // => a b|c",
    ),
    (
        "substr",
        "String",
        "a substring from an offset (negative counts from the end)",
        "echo substr(\"hello\", 1, 3);   // => ell",
    ),
    (
        "strpos",
        "String",
        "first index of a substring, or false if absent",
        "echo strpos(\"hello\", \"l\");   // => 2",
    ),
    (
        "str_replace",
        "String",
        "copy with every occurrence of the search replaced",
        "echo str_replace(\"a\", \"b\", \"aaa\");   // => bbb",
    ),
    (
        "implode",
        "String",
        "join array elements with a separator (alias `join`)",
        "echo implode(\",\", [1, 2, 3]);   // => 1,2,3",
    ),
    (
        "explode",
        "String",
        "split a string on a separator into an array",
        "echo explode(\",\", \"a,b\")[1];   // => b",
    ),
    (
        "str_split",
        "String",
        "split a string into an array of chunks (default length 1)",
        "echo str_split(\"ab\")[0];   // => a",
    ),
    (
        "str_pad",
        "String",
        "pad a string to a length (right pad by default)",
        "echo str_pad(\"5\", 3, \"0\");   // => 500",
    ),
    (
        "str_contains",
        "String",
        "true if the haystack contains the needle",
        "echo str_contains(\"hello\", \"ell\") ? \"y\" : \"n\";   // => y",
    ),
    (
        "str_starts_with",
        "String",
        "true if the string starts with the prefix",
        "echo str_starts_with(\"hello\", \"he\") ? \"y\" : \"n\";   // => y",
    ),
    (
        "str_ends_with",
        "String",
        "true if the string ends with the suffix",
        "echo str_ends_with(\"hello\", \"lo\") ? \"y\" : \"n\";   // => y",
    ),
    (
        "str_word_count",
        "String",
        "number of whitespace-separated words in the string",
        "echo str_word_count(\"a b c\");   // => 3",
    ),
    (
        "number_format",
        "String",
        "format a number with grouped thousands and fixed decimals",
        "echo number_format(1234.567, 2);   // => 1,234.57",
    ),
    (
        "htmlspecialchars",
        "String",
        "escape &, <, >, and quotes for HTML (alias `htmlentities`)",
        "echo htmlspecialchars(\"a<b\");   // => a&lt;b",
    ),
    (
        "strcmp",
        "String",
        "byte comparison of two strings: <0, 0, or >0",
        "echo strcmp(\"a\", \"b\");   // => -1",
    ),
    (
        "strcasecmp",
        "String",
        "case-insensitive comparison of two strings",
        "echo strcasecmp(\"A\", \"a\");   // => 0",
    ),
    (
        "chr",
        "String",
        "one-character string for a byte value (mod 256)",
        "echo chr(65);   // => A",
    ),
    (
        "ord",
        "String",
        "byte value of the first character of a string",
        "echo ord(\"A\");   // => 65",
    ),
    (
        "dechex",
        "String",
        "lowercase hexadecimal string of an integer",
        "echo dechex(255);   // => ff",
    ),
    (
        "hexdec",
        "String",
        "integer value of a hexadecimal string",
        "echo hexdec(\"ff\");   // => 255",
    ),
    (
        "bin2hex",
        "String",
        "hex representation of each byte in a string",
        "echo bin2hex(\"AB\");   // => 4142",
    ),
    (
        "sprintf",
        "String",
        "format arguments into a string using a printf format",
        "echo sprintf(\"%d-%s\", 1, \"x\");   // => 1-x",
    ),
    // ── Array (call_library) ──
    (
        "count",
        "Array",
        "number of elements in an array (alias `sizeof`)",
        "echo count([1, 2, 3]);   // => 3",
    ),
    (
        "array_keys",
        "Array",
        "a new array of the array's keys",
        "echo implode(\",\", array_keys([\"a\" => 1, \"b\" => 2]));   // => a,b",
    ),
    (
        "array_values",
        "Array",
        "a new array of the array's values, reindexed from 0",
        "echo implode(\",\", array_values([5 => \"a\", 9 => \"b\"]));   // => a,b",
    ),
    (
        "array_push",
        "Array",
        "append one or more elements to the end of an array",
        "$a = [1]; array_push($a, 2); echo implode(\",\", $a);   // => 1,2",
    ),
    (
        "in_array",
        "Array",
        "true if a value is present in the array",
        "echo in_array(2, [1, 2, 3]) ? \"y\" : \"n\";   // => y",
    ),
    (
        "range",
        "Array",
        "array of a numeric or character sequence",
        "echo implode(\",\", range(1, 4));   // => 1,2,3,4",
    ),
    (
        "array_merge",
        "Array",
        "concatenate arrays, reindexing integer keys",
        "echo implode(\",\", array_merge([1], [2, 3]));   // => 1,2,3",
    ),
    (
        "array_map",
        "Array",
        "apply a callback to each element, returning a new array",
        "echo implode(\",\", array_map(\"strtoupper\", [\"a\", \"b\"]));   // => A,B",
    ),
    (
        "array_filter",
        "Array",
        "keep elements for which the callback is truthy",
        "echo implode(\",\", array_filter([0, 1, 2]));   // => 1,2",
    ),
    (
        "array_reduce",
        "Array",
        "fold an array to a single value with a callback",
        "echo array_reduce([1, 2, 3], fn($c, $x) => $c + $x, 0);   // => 6",
    ),
    (
        "array_slice",
        "Array",
        "a slice of an array from an offset for a length",
        "echo implode(\",\", array_slice([1, 2, 3, 4], 1, 2));   // => 2,3",
    ),
    (
        "array_reverse",
        "Array",
        "a new array with the elements in reverse order",
        "echo implode(\",\", array_reverse([1, 2, 3]));   // => 3,2,1",
    ),
    (
        "array_sum",
        "Array",
        "sum of the array's numeric values",
        "echo array_sum([1, 2, 3]);   // => 6",
    ),
    (
        "array_product",
        "Array",
        "product of the array's numeric values",
        "echo array_product([2, 3, 4]);   // => 24",
    ),
    (
        "array_flip",
        "Array",
        "a new array with keys and values swapped",
        "echo array_flip([\"a\", \"b\"])[\"a\"];   // => 0",
    ),
    (
        "array_unique",
        "Array",
        "a new array with duplicate values removed",
        "echo implode(\",\", array_unique([1, 1, 2]));   // => 1,2",
    ),
    (
        "array_key_exists",
        "Array",
        "true if the key exists (alias `key_exists`)",
        "echo array_key_exists(\"a\", [\"a\" => 1]) ? \"y\" : \"n\";   // => y",
    ),
    (
        "array_search",
        "Array",
        "the key of the first matching value, or false",
        "echo array_search(2, [1, 2, 3]);   // => 1",
    ),
    (
        "sort",
        "Array",
        "sort an array in ascending order, reindexing keys",
        "$a = [3, 1, 2]; sort($a); echo implode(\",\", $a);   // => 1,2,3",
    ),
    (
        "rsort",
        "Array",
        "sort an array in descending order, reindexing keys",
        "$a = [1, 3, 2]; rsort($a); echo implode(\",\", $a);   // => 3,2,1",
    ),
    (
        "asort",
        "Array",
        "sort by value ascending, preserving key association",
        "$a = [\"b\" => 2, \"a\" => 1]; asort($a); echo implode(\",\", array_keys($a));   // => a,b",
    ),
    (
        "arsort",
        "Array",
        "sort by value descending, preserving key association",
        "$a = [\"a\" => 1, \"b\" => 2]; arsort($a); echo implode(\",\", array_keys($a));   // => b,a",
    ),
    (
        "ksort",
        "Array",
        "sort by key ascending",
        "$a = [\"b\" => 1, \"a\" => 2]; ksort($a); echo implode(\",\", array_keys($a));   // => a,b",
    ),
    (
        "krsort",
        "Array",
        "sort by key descending",
        "$a = [\"a\" => 1, \"b\" => 2]; krsort($a); echo implode(\",\", array_keys($a));   // => b,a",
    ),
    (
        "array_fill",
        "Array",
        "an array of count copies of a value from a start index",
        "echo implode(\",\", array_fill(0, 3, \"x\"));   // => x,x,x",
    ),
    (
        "array_combine",
        "Array",
        "an array pairing one array's values as keys with another's",
        "echo array_combine([\"a\"], [1])[\"a\"];   // => 1",
    ),
    (
        "array_diff",
        "Array",
        "values in the first array absent from the others",
        "echo implode(\",\", array_diff([1, 2, 3], [2]));   // => 1,3",
    ),
    (
        "array_intersect",
        "Array",
        "values present in every array",
        "echo implode(\",\", array_intersect([1, 2, 3], [2, 3, 4]));   // => 2,3",
    ),
    // ── Math (call_library) ──
    (
        "abs",
        "Math",
        "absolute value of a number",
        "echo abs(-5);   // => 5",
    ),
    (
        "floor",
        "Math",
        "largest integer value not greater than x, as a float",
        "echo floor(3.7);   // => 3",
    ),
    (
        "ceil",
        "Math",
        "smallest integer value not less than x, as a float",
        "echo ceil(3.2);   // => 4",
    ),
    (
        "round",
        "Math",
        "round to an optional number of decimal places",
        "echo round(3.14159, 2);   // => 3.14",
    ),
    (
        "sqrt",
        "Math",
        "non-negative square root, as a float",
        "echo sqrt(16);   // => 4",
    ),
    (
        "pow",
        "Math",
        "base raised to an exponent",
        "echo pow(2, 10);   // => 1024",
    ),
    (
        "intdiv",
        "Math",
        "integer division of two integers (truncated)",
        "echo intdiv(7, 2);   // => 3",
    ),
    (
        "fmod",
        "Math",
        "floating-point remainder of x / y",
        "echo fmod(7.5, 2);   // => 1.5",
    ),
    (
        "sin",
        "Math",
        "sine of x (radians)",
        "echo sin(0);   // => 0",
    ),
    (
        "cos",
        "Math",
        "cosine of x (radians)",
        "echo cos(0);   // => 1",
    ),
    (
        "tan",
        "Math",
        "tangent of x (radians)",
        "echo tan(0);   // => 0",
    ),
    (
        "exp",
        "Math",
        "e raised to the power x",
        "echo exp(0);   // => 1",
    ),
    (
        "log",
        "Math",
        "natural log, or log base b when a second arg is given",
        "echo log(8, 2);   // => 3",
    ),
    (
        "log10",
        "Math",
        "base-10 logarithm of x",
        "echo log10(1000);   // => 3",
    ),
    (
        "pi",
        "Math",
        "the constant pi as a float",
        "echo round(pi(), 2);   // => 3.14",
    ),
    (
        "max",
        "Math",
        "largest of the arguments or of an array",
        "echo max(3, 1, 2);   // => 3",
    ),
    (
        "min",
        "Math",
        "smallest of the arguments or of an array",
        "echo min(3, 1, 2);   // => 1",
    ),
    // ── Type (call_library) ──
    (
        "gettype",
        "Type",
        "the type name of a value",
        "echo gettype(1);   // => integer",
    ),
    (
        "is_array",
        "Type",
        "true if the value is an array",
        "echo is_array([1]) ? \"y\" : \"n\";   // => y",
    ),
    (
        "is_int",
        "Type",
        "true if the value is an integer (aliases is_integer/is_long)",
        "echo is_int(5) ? \"y\" : \"n\";   // => y",
    ),
    (
        "is_float",
        "Type",
        "true if the value is a float (alias is_double)",
        "echo is_float(1.5) ? \"y\" : \"n\";   // => y",
    ),
    (
        "is_string",
        "Type",
        "true if the value is a string",
        "echo is_string(\"x\") ? \"y\" : \"n\";   // => y",
    ),
    (
        "is_bool",
        "Type",
        "true if the value is a boolean",
        "echo is_bool(true) ? \"y\" : \"n\";   // => y",
    ),
    (
        "is_null",
        "Type",
        "true if the value is null",
        "echo is_null(null) ? \"y\" : \"n\";   // => y",
    ),
    (
        "is_numeric",
        "Type",
        "true for a number or a numeric string",
        "echo is_numeric(\"3.5\") ? \"y\" : \"n\";   // => y",
    ),
    (
        "is_callable",
        "Type",
        "true if the value names something callable",
        "echo is_callable(\"strlen\") ? \"y\" : \"n\";   // => y",
    ),
    (
        "intval",
        "Type",
        "the integer value of a variable",
        "echo intval(\"42px\");   // => 42",
    ),
    (
        "floatval",
        "Type",
        "the float value of a variable (alias doubleval)",
        "echo floatval(\"3.5kg\");   // => 3.5",
    ),
    (
        "strval",
        "Type",
        "the string value of a variable",
        "echo strval(42);   // => 42",
    ),
    (
        "boolval",
        "Type",
        "the boolean value of a variable",
        "echo boolval(\"\") ? \"y\" : \"n\";   // => n",
    ),
    // ── Output (call_library) ──
    (
        "print_r",
        "Output",
        "human-readable dump of a value; returns it when the 2nd arg is true",
        "echo print_r([1, 2], true);   // => Array ( [0] => 1 [1] => 2 )",
    ),
    (
        "printf",
        "Output",
        "write a printf-formatted string; returns its byte length",
        "printf(\"%d\", 7);   // => 7",
    ),
    (
        "var_dump",
        "Output",
        "dump a value with its type and structure",
        "var_dump(true);   // => bool(true)",
    ),
    (
        "var_export",
        "Output",
        "output (or return) a parsable PHP representation of a value",
        "var_export(1);   // => 1",
    ),
    (
        "json_encode",
        "Output",
        "encode a value as a JSON string",
        "echo json_encode([1, 2, 3]);   // => [1,2,3]",
    ),
];

/// The keyword / builtin corpus, exposed for offline doc generation.
pub fn corpus() -> &'static [(&'static str, &'static str, &'static str, &'static str)] {
    CORPUS
}
