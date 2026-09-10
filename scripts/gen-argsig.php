#!/usr/bin/env php
<?php
/**
 * Regenerate the `SIGS` table at the bottom of `src/argsig.rs` from the
 * reference PHP's own reflection.
 *
 * The set of functions is read from `src/corpus.rs` — the registry every stdlib
 * function is already required to be listed in, and the same one
 * `stdlib::callable::known_builtins` derives `function_exists` from — so this
 * script cannot describe a function the build does not implement, and adding one
 * to the corpus is what puts it here.
 *
 * Run it with the reference binary, not with phplang:
 *
 *     php -d xdebug.mode=off scripts/gen-argsig.php
 *
 * It rewrites everything after the `static SIGS: &[(&str, Sig)] = &[` line in
 * place, and touches nothing above it.
 */

$root = dirname(__DIR__);

// Chapters of the corpus that describe something other than a function.
const NOT_FUNCTIONS = [
    "Predefined constant", "Keyword", "Operator", "Prelude class",
    "Language construct", "Magic method", "Cast", "Built-in object methods",
];

/** Every `(name, chapter)` pair in the corpus, read off its tuple layout. */
function corpus_functions(string $path): array {
    $names = [];
    $pending = null;
    foreach (file($path, FILE_IGNORE_NEW_LINES) as $line) {
        if ($line === "    (") { $pending = false; continue; }
        if ($pending === null) continue;
        if (!preg_match('/^        "((?:[^"\\\\]|\\\\.)*)",/', $line, $m)) continue;
        if ($pending === false) { $pending = $m[1]; continue; }
        if (!in_array($m[1], NOT_FUNCTIONS, true)) $names[] = $pending;
        $pending = null;
    }
    // `exit`/`die` live under "Language construct" but PHP 8.4 made both real
    // functions, and the reference reflects them as such.
    $names[] = "exit";
    $names[] = "die";
    $names = array_values(array_unique($names));
    sort($names);
    return $names;
}

/** `$s` as a Rust string literal. */
function rust_str(string $s): string {
    $o = '"';
    for ($i = 0; $i < strlen($s); $i++) {
        $c = $s[$i]; $b = ord($c);
        if ($c === '\\')      { $o .= '\\\\'; }
        elseif ($c === '"')   { $o .= '\\"'; }
        elseif ($c === "\n")  { $o .= '\\n'; }
        elseif ($c === "\r")  { $o .= '\\r'; }
        elseif ($c === "\t")  { $o .= '\\t'; }
        elseif ($b < 0x20 || $b >= 0x7f) { $o .= sprintf('\\u{%X}', $b); }
        else                  { $o .= $c; }
    }
    return $o . '"';
}

/** The `Def` variant for a parameter, as reflection publishes its default. */
function rust_def(ReflectionParameter $p): string {
    if (!$p->isOptional()) return "Required";
    if (!$p->isDefaultValueAvailable()) return "Unknown";
    $d = $p->getDefaultValue();
    if ($d === null)   return "Null";
    if (is_bool($d))   return "Bool(" . ($d ? "true" : "false") . ")";
    if (is_int($d))    return "Int($d)";
    if (is_float($d))  return (is_nan($d) || is_infinite($d)) ? "Unknown" : "Float(" . var_export($d, true) . ")";
    if (is_string($d)) return "Str(" . rust_str($d) . ")";
    if (is_array($d))  return count($d) === 0 ? "EmptyArray" : "Unknown";
    // An object default (`round`'s RoundingMode enum) has no literal here; the
    // slot is last in its signature, so no gap can ever need the value.
    return "Unknown";
}

$rows = [];
$skipped = [];
foreach (corpus_functions("$root/src/corpus.rs") as $name) {
    if (!function_exists($name)) { $skipped[] = $name; continue; }
    $r = new ReflectionFunction($name);
    $params = [];
    $variadic = false;
    foreach ($r->getParameters() as $p) {
        if ($p->isVariadic()) { $variadic = true; continue; }
        $t = $p->getType();
        $params[] = sprintf(
            "p(%s, %s, %s)",
            rust_str($p->getName()),
            rust_def($p),
            rust_str($t ? (string) $t : "")
        );
    }
    $rows[] = sprintf(
        "    (%s, Sig { req: %d, params: &[%s], variadic: %s }),",
        rust_str(strtolower($name)),
        $r->getNumberOfRequiredParameters(),
        implode(", ", $params),
        $variadic ? "true" : "false"
    );
}

$target = "$root/src/argsig.rs";
$src = file_get_contents($target);
$marker = "static SIGS: &[(&str, Sig)] = &[\n";
$at = strpos($src, $marker);
if ($at === false) { fwrite(STDERR, "no SIGS marker in $target\n"); exit(1); }
$head = substr($src, 0, $at + strlen($marker));
file_put_contents($target, $head . "    // generated: " . count($rows) . " functions\n" . implode("\n", $rows) . "\n];\n");

fwrite(STDERR, sprintf(
    "wrote %d signatures to src/argsig.rs (reference: %s)\nnot in reference PHP, skipped: %s\n",
    count($rows), PHP_VERSION, implode(", ", $skipped)
));
