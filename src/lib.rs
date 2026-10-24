//! phplang — PHP as a fusevm frontend.
//!
//! Pipeline: `lexer` → `parser` builds a PHP AST → `compiler` lowers it to a
//! `fusevm::Chunk` (plus a table of function sub-chunks) → fusevm executes it,
//! calling back into the `host` (through registered builtins and the strict
//! numeric hook) for every PHP-specific operation. There is no bespoke VM or
//! JIT here — execution and codegen live in fusevm.

pub mod argsig;
pub mod argtypes;
pub mod ast;
pub mod banner;
pub mod builtins;
pub mod bytes;
pub mod cli;
pub mod compiler;
pub mod corpus;
pub mod dap;
pub mod errlevel;
pub mod host;
pub mod intercepts;
pub mod internal_interfaces;
pub mod lexer;
pub mod lsp;
pub mod parser;
pub mod promote;
pub mod repl;
pub mod rust_ffi;
pub mod stdlib;
pub mod tiers;
pub mod timelib;

pub use fusevm::Value;

/// Compile a PHP source string to a runnable program.
pub fn compile(src: &str) -> Result<compiler::Program, String> {
    compile_with_meta(src, false)
}

/// Compile with per-statement DAP line markers enabled (`php --dap`).
pub fn compile_debug(src: &str) -> Result<compiler::Program, String> {
    compile_with_meta(src, true)
}

/// The shared body of [`compile`] and [`compile_debug`].
///
/// The file-level facts the parse turns up have to be applied HERE rather than only
/// on the CLI path: `declare(strict_types=1)` changes how every later call binds its
/// arguments, so a program compiled through the library API would otherwise run in
/// coercive mode however it was written.
fn compile_with_meta(src: &str, debug: bool) -> Result<compiler::Program, String> {
    // A declaration is checked against the prelude's types, which are only
    // known once the prelude has compiled.
    prelude_defs();
    let (stmts, meta) = parser::parse_meta(src).map_err(|e| e.message)?;
    host::with_host(|h| {
        for (line, msg) in &meta.declare_warnings {
            h.warn_at(msg, *line);
        }
        h.set_strict_types(meta.strict_types);
    });
    compiler::compile(&stmts, debug)
}

/// The built-in exception class hierarchy, written in PHP so it flows through the
/// real class system: `throw`/`catch` resolve these as ordinary classes, and user
/// code can subclass them. `Exception` and `Error` are the two disjoint roots
/// (`catch (Throwable)` — handled in the host — matches either); the rest inherit
/// their `__construct`/`getMessage`/`getCode`/`getPrevious`/`__toString`.
///
/// `file`, `line` and `trace` are filled in by `host::seed_throwable` at
/// construction, exactly when the reference engine records them, so `getFile`,
/// `getLine` and `getTraceAsString` report the `new` site rather than the
/// `throw` site. `trace` is the structured frame array `getTrace()` returns;
/// `getTraceAsString()` renders it.
const EXCEPTION_PRELUDE: &str = r#"<?php
class Exception implements Throwable {
    protected $message = "";
    private $string = "";
    protected $code = 0;
    protected $file = "";
    protected $line = 0;
    private $trace = [];
    private $previous = null;
    public function __construct($message = "", $code = 0, $previous = null) {
        $this->message = $message;
        $this->code = $code;
        $this->previous = $previous;
    }
    final public function getMessage() { return $this->message; }
    final public function getCode() { return $this->code; }
    final public function getPrevious() { return $this->previous; }
    final public function getFile() { return $this->file; }
    final public function getLine() { return $this->line; }
    final public function getTrace() { return $this->trace; }
    final public function getTraceAsString() { return __phplang_trace_string($this->trace); }
    public function __toString() { return __phplang_throwable_string($this); }
}
class Error implements Throwable {
    protected $message = "";
    private $string = "";
    protected $code = 0;
    protected $file = "";
    protected $line = 0;
    private $trace = [];
    private $previous = null;
    public function __construct($message = "", $code = 0, $previous = null) {
        $this->message = $message;
        $this->code = $code;
        $this->previous = $previous;
    }
    final public function getMessage() { return $this->message; }
    final public function getCode() { return $this->code; }
    final public function getPrevious() { return $this->previous; }
    final public function getFile() { return $this->file; }
    final public function getLine() { return $this->line; }
    final public function getTrace() { return $this->trace; }
    final public function getTraceAsString() { return __phplang_trace_string($this->trace); }
    public function __toString() { return __phplang_throwable_string($this); }
}
class ErrorException extends Exception {
    protected $severity = 1;
    public function __construct($message = "", $code = 0, $severity = 1, $filename = null, $line = null, $previous = null) {
        parent::__construct($message, $code, $previous);
        $this->severity = $severity;
        // A filename replaces the recorded line too — with 0 when none is
        // given — while a line alone replaces only the line
        // (`ErrorException::__construct`, `Zend/zend_exceptions.c`).
        if ($filename !== null) {
            $this->file = $filename;
            $this->line = $line === null ? 0 : $line;
        } elseif ($line !== null) {
            $this->line = $line;
        }
    }
    final public function getSeverity() { return $this->severity; }
}
class RuntimeException extends Exception {}
class LogicException extends Exception {}
class InvalidArgumentException extends LogicException {}
class AssertionError extends Error {}
class ArithmeticError extends Error {}
class DivisionByZeroError extends ArithmeticError {}
class TypeError extends Error {}
class ArgumentCountError extends TypeError {}
class ValueError extends Error {}
class JsonException extends Exception {}
class CompileError extends Error {}
class ParseError extends CompileError {}
class UnhandledMatchError extends Error {}
class BadFunctionCallException extends LogicException {}
class BadMethodCallException extends BadFunctionCallException {}
class DomainException extends LogicException {}
class LengthException extends LogicException {}
class OutOfRangeException extends LogicException {}
class OutOfBoundsException extends RuntimeException {}
class OverflowException extends RuntimeException {}
class RangeException extends RuntimeException {}
class UnderflowException extends RuntimeException {}
class UnexpectedValueException extends RuntimeException {}
"#;

/// The date/time classes. Each method is a thin PHP shell over a
/// `__phplang_date_*` helper in `stdlib::datefn`, which ports the matching
/// `php_date.c` routine over `crate::timelib`; an object's state is the
/// properties the reference shows for it (see `stdlib::datefn`). Appended to
/// the prelude source (no `<?php` tag — it is concatenated after the
/// exceptions).
const DATETIME_PRELUDE: &str = r#"
class DateError extends Error {}
class DateObjectError extends DateError {}
class DateRangeError extends DateError {}
class DateException extends Exception {}
class DateInvalidTimeZoneException extends DateException {}
class DateInvalidOperationException extends DateException {}
class DateMalformedStringException extends DateException {}
class DateMalformedIntervalStringException extends DateException {}
class DateMalformedPeriodStringException extends DateException {}
interface DateTimeInterface {
    const ATOM = 'Y-m-d\TH:i:sP';
    const COOKIE = 'l, d-M-Y H:i:s T';
    const ISO8601 = 'Y-m-d\TH:i:sO';
    const ISO8601_EXPANDED = 'X-m-d\TH:i:sP';
    const RFC822 = 'D, d M y H:i:s O';
    const RFC850 = 'l, d-M-y H:i:s T';
    const RFC1036 = 'D, d M y H:i:s O';
    const RFC1123 = 'D, d M Y H:i:s O';
    const RFC7231 = 'D, d M Y H:i:s \G\M\T';
    const RFC2822 = 'D, d M Y H:i:s O';
    const RFC3339 = 'Y-m-d\TH:i:sP';
    const RFC3339_EXTENDED = 'Y-m-d\TH:i:s.vP';
    const RSS = 'D, d M Y H:i:s O';
    const W3C = 'Y-m-d\TH:i:sP';
    public function format(string $format): string;
    public function getTimezone(): DateTimeZone|false;
    public function getOffset(): int;
    public function getTimestamp(): int;
    public function getMicrosecond(): int;
    public function diff(DateTimeInterface $targetObject, bool $absolute = false): DateInterval;
}
class DateTime implements DateTimeInterface {
    public function __construct(string $datetime = "now", ?DateTimeZone $timezone = null) { __phplang_date_init($this, $datetime, $timezone); }
    public function format(string $format): string { return __phplang_date_format($this, $format); }
    public function modify(string $modifier): DateTime { return __phplang_date_modify($this, $modifier); }
    public function add(DateInterval $interval): DateTime { return __phplang_date_add($this, $interval); }
    public function sub(DateInterval $interval): DateTime { return __phplang_date_sub($this, $interval); }
    public function diff(DateTimeInterface $targetObject, bool $absolute = false): DateInterval { return __phplang_date_diff($this, $targetObject, $absolute); }
    public function getTimestamp(): int { return __phplang_date_timestamp($this); }
    public function getMicrosecond(): int { return __phplang_date_microsecond($this); }
    public function getOffset(): int { return __phplang_date_offset($this); }
    public function getTimezone(): DateTimeZone|false { return __phplang_date_timezone_get($this); }
    public function setTimezone(DateTimeZone $timezone): DateTime { return __phplang_date_timezone_set($this, $timezone); }
    public function setDate(int $year, int $month, int $day): DateTime { return __phplang_date_set($this, "date", $year, $month, $day, 0); }
    public function setISODate(int $year, int $week, int $dayOfWeek = 1): DateTime { return __phplang_date_set($this, "isodate", $year, $week, $dayOfWeek, 0); }
    public function setTime(int $hour, int $minute, int $second = 0, int $microsecond = 0): DateTime { return __phplang_date_set($this, "time", $hour, $minute, $second, $microsecond); }
    public function setTimestamp(int $timestamp): DateTime { return __phplang_date_set($this, "timestamp", $timestamp, 0, 0, 0); }
    public function setMicrosecond(int $microsecond): static { return __phplang_date_set($this, "microsecond", $microsecond, 0, 0, 0); }
    public static function createFromFormat(string $format, string $datetime, ?DateTimeZone $timezone = null): DateTime|false {
        $o = __phplang_date_new(static::class);
        return __phplang_date_init($o, $datetime, $timezone, $format) ? $o : false;
    }
    public static function createFromImmutable(DateTimeImmutable $object): static { return __phplang_date_copy(__phplang_date_new(static::class), $object); }
    public static function createFromInterface(DateTimeInterface $object): DateTime { return __phplang_date_copy(__phplang_date_new(static::class), $object); }
    public static function createFromTimestamp(int|float $timestamp): static { return __phplang_date_from_timestamp(__phplang_date_new(static::class), $timestamp); }
    public static function getLastErrors(): array|false { return __phplang_date_last_errors(); }
}
class DateTimeImmutable implements DateTimeInterface {
    public function __construct(string $datetime = "now", ?DateTimeZone $timezone = null) { __phplang_date_init($this, $datetime, $timezone); }
    public function format(string $format): string { return __phplang_date_format($this, $format); }
    public function modify(string $modifier): DateTimeImmutable { return __phplang_date_modify(clone $this, $modifier); }
    public function add(DateInterval $interval): DateTimeImmutable { return __phplang_date_add(clone $this, $interval); }
    public function sub(DateInterval $interval): DateTimeImmutable { return __phplang_date_sub(clone $this, $interval); }
    public function diff(DateTimeInterface $targetObject, bool $absolute = false): DateInterval { return __phplang_date_diff($this, $targetObject, $absolute); }
    public function getTimestamp(): int { return __phplang_date_timestamp($this); }
    public function getMicrosecond(): int { return __phplang_date_microsecond($this); }
    public function getOffset(): int { return __phplang_date_offset($this); }
    public function getTimezone(): DateTimeZone|false { return __phplang_date_timezone_get($this); }
    public function setTimezone(DateTimeZone $timezone): DateTimeImmutable { return __phplang_date_timezone_set(clone $this, $timezone); }
    public function setDate(int $year, int $month, int $day): DateTimeImmutable { return __phplang_date_set(clone $this, "date", $year, $month, $day, 0); }
    public function setISODate(int $year, int $week, int $dayOfWeek = 1): DateTimeImmutable { return __phplang_date_set(clone $this, "isodate", $year, $week, $dayOfWeek, 0); }
    public function setTime(int $hour, int $minute, int $second = 0, int $microsecond = 0): DateTimeImmutable { return __phplang_date_set(clone $this, "time", $hour, $minute, $second, $microsecond); }
    public function setTimestamp(int $timestamp): DateTimeImmutable { return __phplang_date_set(clone $this, "timestamp", $timestamp, 0, 0, 0); }
    public function setMicrosecond(int $microsecond): static { return __phplang_date_set(clone $this, "microsecond", $microsecond, 0, 0, 0); }
    public static function createFromFormat(string $format, string $datetime, ?DateTimeZone $timezone = null): DateTimeImmutable|false {
        $o = __phplang_date_new(static::class);
        return __phplang_date_init($o, $datetime, $timezone, $format) ? $o : false;
    }
    public static function createFromMutable(DateTime $object): static { return __phplang_date_copy(__phplang_date_new(static::class), $object); }
    public static function createFromInterface(DateTimeInterface $object): DateTimeImmutable { return __phplang_date_copy(__phplang_date_new(static::class), $object); }
    public static function createFromTimestamp(int|float $timestamp): static { return __phplang_date_from_timestamp(__phplang_date_new(static::class), $timestamp); }
    public static function getLastErrors(): array|false { return __phplang_date_last_errors(); }
}
class DateTimeZone {
    const AFRICA = 1;
    const AMERICA = 2;
    const ANTARCTICA = 4;
    const ARCTIC = 8;
    const ASIA = 16;
    const ATLANTIC = 32;
    const AUSTRALIA = 64;
    const EUROPE = 128;
    const INDIAN = 256;
    const PACIFIC = 512;
    const UTC = 1024;
    const ALL = 2047;
    const ALL_WITH_BC = 4095;
    const PER_COUNTRY = 4096;
    public function __construct(string $timezone) { __phplang_tz_init($this, $timezone); }
    public function getName(): string { return __phplang_tz_name($this); }
    public function getOffset(DateTimeInterface $datetime): int { return __phplang_tz_offset($this, $datetime); }
}
class DatePeriod implements IteratorAggregate {
    const EXCLUDE_START_DATE = 1;
    const INCLUDE_END_DATE = 2;
    public function __construct($start, $interval = null, $end = null, $options = null) { __phplang_period_init($this, func_get_args()); }
    public function getStartDate(): DateTimeInterface { return clone $this->start; }
    public function getEndDate(): ?DateTimeInterface { return $this->end === null ? null : clone $this->end; }
    public function getDateInterval(): DateInterval { return clone $this->interval; }
    public function getRecurrences(): ?int {
        $n = $this->recurrences - (int) $this->include_start_date - (int) $this->include_end_date;
        return $n === 0 ? null : $n;
    }
    public function getIterator(): Iterator { return new ArrayIterator(__phplang_period_list($this)); }
}
class DateInterval {
    public function __construct(string $duration) { __phplang_interval_init($this, $duration); }
    public function format(string $format): string { return __phplang_interval_format($this, $format); }
    public static function createFromDateString(string $datetime): DateInterval { return __phplang_interval_from_string($datetime); }
    public function __get($name) { return __phplang_interval_get($this, $name); }
}
"#;

/// `ext/hash`'s one class. The state `hash_init` … `hash_final`
/// (`stdlib::hashext`) keep in it is the private properties; `__debugInfo`
/// shows the `algo` the reference's `var_dump` shows. The constructor is
/// private as the reference's is: a context comes only from `hash_init` or
/// `hash_copy`.
const HASH_PRELUDE: &str = r#"
final class HashContext {
    private string $algo = "";
    private ?string $key = null;
    private ?int $seed = null;
    private ?string $secret = null;
    private string $data = "";
    private bool $finalized = false;
    private function __construct() {}
    public function __debugInfo(): array { return ["algo" => $this->algo]; }
}
"#;

/// `Zend/zend_weakrefs.c`'s `WeakReference` and `WeakMap`. Their state lives in
/// static tables keyed by object id, so an instance has no properties of its
/// own, as the reference's have none. phplang has no refcounts, so a referent
/// is never released early: `get()` keeps answering it and a map keeps its
/// entry after the program drops the key (BUGS.md).
const WEAKREF_PRELUDE: &str = r##"
final class WeakReference {
    // referent id => the WeakReference made for it; WeakReference id => referent
    private static $__made = [];
    private static $__referent = [];
    private static $__creating = false;
    public function __construct() {
        if (!self::$__creating) { throw new Error("Direct instantiation of WeakReference is not allowed, use WeakReference::create instead"); }
    }
    // `zend_weakref_find`: one WeakReference per referent.
    public static function create(object $object): WeakReference {
        $id = spl_object_id($object);
        if (isset(self::$__made[$id])) { return self::$__made[$id]; }
        self::$__creating = true;
        $ref = new WeakReference();
        self::$__creating = false;
        self::$__referent[spl_object_id($ref)] = $object;
        return self::$__made[$id] = $ref;
    }
    public function get(): ?object { return self::$__referent[spl_object_id($this)] ?? null; }
    public function __debugInfo(): array { return ["object" => $this->get()]; }
}
final class WeakMap implements ArrayAccess, Countable, IteratorAggregate {
    // map id => [key id => [key, value]]
    private static $__maps = [];
    private static function __key($object) {
        if (!is_object($object)) { throw new TypeError("WeakMap key must be an object"); }
        return spl_object_id($object);
    }
    public function offsetGet($object): mixed {
        $k = self::__key($object);
        $map = self::$__maps[spl_object_id($this)] ?? [];
        if (!array_key_exists($k, $map)) { throw new Error("Object " . get_class($object) . "#$k not contained in WeakMap"); }
        return $map[$k][1];
    }
    // A null offset is `$map[] = $v`, which `zend_weakmap_write_dimension` refuses.
    public function offsetSet($object, mixed $value): void {
        if ($object === null) { throw new Error("Cannot append to WeakMap"); }
        self::$__maps[spl_object_id($this)][self::__key($object)] = [$object, $value];
    }
    // `zend_weakmap_has_dimension`: present and not null.
    public function offsetExists($object): bool {
        return isset((self::$__maps[spl_object_id($this)] ?? [])[self::__key($object)][1]);
    }
    public function offsetUnset($object): void {
        unset(self::$__maps[spl_object_id($this)][self::__key($object)]);
    }
    public function count(): int { return count(self::$__maps[spl_object_id($this)] ?? []); }
    public function getIterator(): Iterator {
        foreach (self::$__maps[spl_object_id($this)] ?? [] as [$key, $value]) { yield $key => $value; }
    }
    private function __phplang_clone($orig) { self::$__maps[spl_object_id($this)] = self::$__maps[spl_object_id($orig)] ?? []; }
    public function __debugInfo(): array {
        $out = [];
        foreach (self::$__maps[spl_object_id($this)] ?? [] as [$key, $value]) { $out[] = ["key" => $key, "value" => $value]; }
        return $out;
    }
}
"##;

/// The Reflection API (`ext/reflection`), written in PHP over the engine's
/// `__phplang_reflect` metadata. Each object keeps only the public `name`
/// (and `class`) the reference shows; the rest of its state lives in static
/// tables keyed by object id, as `WeakReference`'s does. Access that crosses
/// visibility (`getValue`, `invoke`) runs in a closure bound to the declaring
/// class, which is the scope the reference grants it.
const REFLECTION_PRELUDE: &str = r##"
class ReflectionException extends Exception {}

class Reflection {
    public static function getModifierNames(int $modifiers): array {
        $names = [];
        if ($modifiers & 64) { $names[] = "abstract"; }
        if ($modifiers & 32) { $names[] = "final"; }
        if ($modifiers & 1) { $names[] = "public"; }
        elseif ($modifiers & 4) { $names[] = "private"; }
        elseif ($modifiers & 2) { $names[] = "protected"; }
        if ($modifiers & 1024) { $names[] = "public(set)"; }
        elseif ($modifiers & 2048) { $names[] = "protected(set)"; }
        elseif ($modifiers & 4096) { $names[] = "private(set)"; }
        if ($modifiers & 16) { $names[] = "static"; }
        if ($modifiers & (128 | 65536)) { $names[] = "readonly"; }
        return $names;
    }
}

abstract class ReflectionType implements Stringable {
    // object id => [name => as printed, parts => alternatives, null => allows null]
    protected static $__info = [];
    public static function __make(array $info): ReflectionType {
        $t = count($info["parts"]) == 1 ? new ReflectionNamedType() : new ReflectionUnionType();
        ReflectionType::$__info[spl_object_id($t)] = $info;
        return $t;
    }
    public function allowsNull(): bool { return ReflectionType::$__info[spl_object_id($this)]["null"]; }
    public function __toString(): string { return ReflectionType::$__info[spl_object_id($this)]["name"]; }
}

class ReflectionNamedType extends ReflectionType {
    public function getName(): string { return ReflectionType::$__info[spl_object_id($this)]["parts"][0]; }
    public function isBuiltin(): bool {
        return in_array(strtolower($this->getName()), ["int", "float", "string", "bool", "array", "mixed",
            "void", "null", "never", "false", "true", "callable", "iterable", "object", "static"], true);
    }
}

class ReflectionUnionType extends ReflectionType {
    public function getTypes(): array {
        $info = ReflectionType::$__info[spl_object_id($this)];
        $types = [];
        foreach ($info["parts"] as $p) { $types[] = ReflectionType::__make(["name" => $p, "parts" => [$p], "null" => $p === "null"]); }
        if ($info["null"]) { $types[] = ReflectionType::__make(["name" => "null", "parts" => ["null"], "null" => true]); }
        return $types;
    }
}

abstract class ReflectionFunctionAbstract {
    public string $name = "";
    // object id => [kind, target, name] — what __phplang_reflect("params") reads
    protected static $__spec = [];
    public function __spec(): array { return ReflectionFunctionAbstract::$__spec[spl_object_id($this)]; }
    public function getName(): string { return $this->name; }
    public function getShortName(): string { $p = strrpos($this->name, "\\"); return $p === false ? $this->name : substr($this->name, $p + 1); }
    public function getNamespaceName(): string { $p = strrpos($this->name, "\\"); return $p === false ? "" : substr($this->name, 0, $p); }
    public function inNamespace(): bool { return str_contains($this->name, "\\"); }
    public function getParameters(): array {
        [$kind, $target, $fname] = $this->__spec();
        $out = [];
        foreach (__phplang_reflect("params", $kind, $target, $fname) as $i => $info) {
            $out[] = ReflectionParameter::__make($this, $info, $i);
        }
        return $out;
    }
    public function getNumberOfParameters(): int { [$k, $t, $n] = $this->__spec(); return count(__phplang_reflect("params", $k, $t, $n)); }
    public function getNumberOfRequiredParameters(): int {
        [$k, $t, $n] = $this->__spec();
        $req = 0;
        foreach (__phplang_reflect("params", $k, $t, $n) as $p) { if (!$p["optional"]) { $req++; } }
        return $req;
    }
    public function isVariadic(): bool {
        [$k, $t, $n] = $this->__spec();
        foreach (__phplang_reflect("params", $k, $t, $n) as $p) { if ($p["variadic"]) { return true; } }
        return false;
    }
    abstract protected function __returnInfo();
    public function hasReturnType(): bool { return $this->__returnInfo() !== null; }
    public function getReturnType(): ?ReflectionType { $t = $this->__returnInfo(); return $t === null ? null : ReflectionType::__make($t); }
    public function getDocComment(): string|false { [$k, $t, $n] = $this->__spec(); return __phplang_reflect("doc", $k, $t, $n); }
    public function isUserDefined(): bool { return !$this->isInternal(); }
    public function getStaticVariables(): array { [$k, $t, $n] = $this->__spec(); return $k === "method" ? __phplang_reflect("statics", $t, $n) : []; }
    public function getAttributes(?string $name = null, int $flags = 0): array {
        $spec = $this->__spec();
        return ReflectionAttribute::__list("ReflectionFunctionAbstract::getAttributes", $spec, -1, $spec[0] === "method" ? Attribute::TARGET_METHOD : Attribute::TARGET_FUNCTION, $name, $flags);
    }
}

class ReflectionFunction extends ReflectionFunctionAbstract {
    // object id => what __phplang_reflect("function") answered
    private static $__fn = [];
    public function __construct(Closure|string $function) {
        $info = __phplang_reflect("function", $function);
        if ($info === false) { throw new ReflectionException("Function " . $function . "() does not exist"); }
        $this->name = $info["name"];
        ReflectionFunction::$__fn[spl_object_id($this)] = [$info, $function];
        ReflectionFunctionAbstract::$__spec[spl_object_id($this)] = $info["closure"] ? ["closure", $function, ""] : ["function", $info["name"], ""];
    }
    protected function __returnInfo() { return ReflectionFunction::$__fn[spl_object_id($this)][0]["return"]; }
    public function isInternal(): bool { return ReflectionFunction::$__fn[spl_object_id($this)][0]["internal"]; }
    public function isClosure(): bool { return ReflectionFunction::$__fn[spl_object_id($this)][0]["closure"]; }
    public function invoke(mixed ...$args): mixed { return call_user_func_array(ReflectionFunction::$__fn[spl_object_id($this)][1], $args); }
    public function invokeArgs(array $args = []): mixed { return call_user_func_array(ReflectionFunction::$__fn[spl_object_id($this)][1], $args); }
    public function getClosure(): Closure { $f = ReflectionFunction::$__fn[spl_object_id($this)][1]; return $f instanceof Closure ? $f : Closure::fromCallable($f); }
    public function getStaticVariables(): array { [$k, $t] = $this->__spec(); return __phplang_reflect("statics", $t); }
}

class ReflectionMethod extends ReflectionFunctionAbstract {
    const IS_STATIC = 16;
    const IS_PUBLIC = 1;
    const IS_PROTECTED = 2;
    const IS_PRIVATE = 4;
    const IS_ABSTRACT = 64;
    const IS_FINAL = 32;
    public string $class = "";
    private static $__m = [];
    public function __construct(object|string $objectOrMethod, ?string $method = null) {
        if ($method === null) {
            __phplang_reflect("deprecated", "Calling ReflectionMethod::__construct() with 1 argument is deprecated, use ReflectionMethod::createFromMethodName() instead");
            if (!is_string($objectOrMethod) || !str_contains($objectOrMethod, "::")) {
                throw new ReflectionException("ReflectionMethod::__construct(): Argument #1 (\$objectOrMethod) must be a valid method name");
            }
            [$objectOrMethod, $method] = explode("::", $objectOrMethod, 2);
        }
        $class = is_object($objectOrMethod) ? get_class($objectOrMethod) : $objectOrMethod;
        $c = __phplang_reflect("class", $class);
        if ($c === false) { throw new ReflectionException("Class \"" . $class . "\" does not exist"); }
        $info = __phplang_reflect("method", $c["name"], $method);
        if ($info === null) { throw new ReflectionException("Method " . $c["name"] . "::" . $method . "() does not exist"); }
        $this->name = $info["name"];
        $this->class = $info["class"];
        ReflectionMethod::$__m[spl_object_id($this)] = $info;
        ReflectionFunctionAbstract::$__spec[spl_object_id($this)] = ["method", $info["class"], $info["name"]];
    }
    public static function createFromMethodName(string $method): static {
        if (!str_contains($method, "::")) {
            throw new ReflectionException("ReflectionMethod::createFromMethodName(): Argument #1 (\$method) must be a valid method name");
        }
        [$class, $name] = explode("::", $method, 2);
        return new static($class, $name);
    }
    private function __info(): array { return ReflectionMethod::$__m[spl_object_id($this)]; }
    protected function __returnInfo() { return $this->__info()["type"]; }
    public function getModifiers(): int { return $this->__info()["modifiers"]; }
    public function isPublic(): bool { return ($this->getModifiers() & 1) != 0; }
    public function isProtected(): bool { return ($this->getModifiers() & 2) != 0; }
    public function isPrivate(): bool { return ($this->getModifiers() & 4) != 0; }
    public function isStatic(): bool { return ($this->getModifiers() & 16) != 0; }
    public function isFinal(): bool { return ($this->getModifiers() & 32) != 0; }
    public function isAbstract(): bool { return ($this->getModifiers() & 64) != 0; }
    public function isConstructor(): bool { return strcasecmp($this->name, "__construct") == 0; }
    public function isDestructor(): bool { return strcasecmp($this->name, "__destruct") == 0; }
    public function isInternal(): bool { return __phplang_reflect("class", $this->class)["internal"]; }
    public function getDeclaringClass(): ReflectionClass { return new ReflectionClass($this->class); }
    #[\Deprecated(message: "as it has no effect since PHP 8.1", since: "8.5")]
    public function setAccessible(bool $accessible): void {}
    public function invoke(?object $object = null, mixed ...$args): mixed { return $this->invokeArgs($object, $args); }
    public function invokeArgs(?object $object = null, array $args = []): mixed {
        $name = $this->name;
        if ($this->isStatic()) {
            return Closure::bind(function ($c, $n, $a) { return $c::$n(...$a); }, null, $this->class)($this->class, $name, $args);
        }
        return Closure::bind(function ($o, $n, $a) { return $o->$n(...$a); }, null, $this->class)($object, $name, $args);
    }
    public function getClosure(?object $object = null): Closure {
        $name = $this->name;
        if ($this->isStatic()) {
            return Closure::bind(function (...$a) use ($name) { return static::$name(...$a); }, null, $this->class);
        }
        return Closure::bind(function (...$a) use ($name) { return $this->$name(...$a); }, $object, $this->class);
    }
}

class ReflectionParameter {
    public string $name = "";
    // object id => [function, info, position]
    private static $__p = [];
    public static function __make(ReflectionFunctionAbstract $fn, array $info, int $position): ReflectionParameter {
        $p = new ReflectionParameter();
        $p->name = $info["name"];
        ReflectionParameter::$__p[spl_object_id($p)] = [$fn, $info, $position];
        return $p;
    }
    private function __info(): array { return ReflectionParameter::$__p[spl_object_id($this)][1]; }
    public function getName(): string { return $this->name; }
    public function getPosition(): int { return ReflectionParameter::$__p[spl_object_id($this)][2]; }
    public function isOptional(): bool { return $this->__info()["optional"]; }
    public function isVariadic(): bool { return $this->__info()["variadic"]; }
    public function isPassedByReference(): bool { return $this->__info()["byref"]; }
    public function canBePassedByValue(): bool { return !$this->__info()["byref"]; }
    public function hasType(): bool { return $this->__info()["type"] !== null; }
    public function getType(): ?ReflectionType { $t = $this->__info()["type"]; return $t === null ? null : ReflectionType::__make($t); }
    public function allowsNull(): bool { $t = $this->__info()["type"]; return $t === null || $t["null"]; }
    public function isDefaultValueAvailable(): bool { return $this->__info()["default"]; }
    public function getDefaultValue(): mixed {
        if (!$this->__info()["default"]) { throw new ReflectionException("Internal error: Failed to retrieve the default value"); }
        $fn = ReflectionParameter::$__p[spl_object_id($this)][0];
        [$k, $t, $n] = $fn->__spec();
        return __phplang_reflect("default", $k, $t, $n, $this->getPosition());
    }
    public function getDeclaringFunction(): ReflectionFunctionAbstract { return ReflectionParameter::$__p[spl_object_id($this)][0]; }
    public function getDeclaringClass(): ?ReflectionClass {
        $fn = ReflectionParameter::$__p[spl_object_id($this)][0];
        return $fn instanceof ReflectionMethod ? new ReflectionClass($fn->class) : null;
    }
    public function getAttributes(?string $name = null, int $flags = 0): array { $p = ReflectionParameter::$__p[spl_object_id($this)]; return ReflectionAttribute::__list("ReflectionParameter::getAttributes", $p[0]->__spec(), $p[2], Attribute::TARGET_PARAMETER, $name, $flags); }
}

class ReflectionProperty {
    const IS_STATIC = 16;
    const IS_READONLY = 128;
    const IS_PUBLIC = 1;
    const IS_PROTECTED = 2;
    const IS_PRIVATE = 4;
    const IS_ABSTRACT = 64;
    const IS_FINAL = 32;
    public string $name = "";
    public string $class = "";
    private static $__p = [];
    public function __construct(object|string $class, string $property) {
        $cname = is_object($class) ? get_class($class) : $class;
        $c = __phplang_reflect("class", $cname);
        if ($c === false) { throw new ReflectionException("Class \"" . $cname . "\" does not exist"); }
        $info = __phplang_reflect("prop", $c["name"], $property);
        if ($info === null && is_object($class) && property_exists($class, $property)) {
            $info = ["name" => $property, "class" => $c["name"], "modifiers" => 1, "type" => null, "default" => false];
        }
        if ($info === null) { throw new ReflectionException("Property " . $c["name"] . "::\$" . $property . " does not exist"); }
        $this->name = $info["name"];
        $this->class = $info["class"];
        ReflectionProperty::$__p[spl_object_id($this)] = $info;
    }
    private function __info(): array { return ReflectionProperty::$__p[spl_object_id($this)]; }
    public function getName(): string { return $this->name; }
    public function getModifiers(): int { return $this->__info()["modifiers"]; }
    public function isPublic(): bool { return ($this->getModifiers() & 1) != 0; }
    public function isProtected(): bool { return ($this->getModifiers() & 2) != 0; }
    public function isPrivate(): bool { return ($this->getModifiers() & 4) != 0; }
    public function isStatic(): bool { return ($this->getModifiers() & 16) != 0; }
    public function isReadOnly(): bool { return ($this->getModifiers() & 128) != 0; }
    public function isFinal(): bool { return ($this->getModifiers() & 32) != 0; }
    public function isDefault(): bool { return true; }
    public function hasType(): bool { return $this->__info()["type"] !== null; }
    public function getType(): ?ReflectionType { $t = $this->__info()["type"]; return $t === null ? null : ReflectionType::__make($t); }
    public function hasDefaultValue(): bool { return $this->__info()["default"]; }
    public function getDefaultValue(): mixed { return __phplang_reflect("prop_default", $this->class, $this->name); }
    public function getDeclaringClass(): ReflectionClass { return new ReflectionClass($this->class); }
    public function getDocComment(): string|false { return __phplang_reflect("doc", "prop", $this->class, $this->name); }
    #[\Deprecated(message: "as it has no effect since PHP 8.1", since: "8.5")]
    public function setAccessible(bool $accessible): void {}
    public function getValue(?object $object = null): mixed {
        if ($this->isStatic()) {
            return Closure::bind(function ($c, $n) { return __phplang_reflect("static_get", $c, $n); }, null, $this->class)($this->class, $this->name);
        }
        return Closure::bind(function ($o, $n) { return $o->$n; }, null, $this->class)($object, $this->name);
    }
    public function setValue(mixed $objectOrValue = null, mixed $value = null): void {
        if ($this->isStatic()) {
            if (func_num_args() == 1) {
                __phplang_reflect("deprecated", "Calling ReflectionProperty::setValue() with a single argument is deprecated");
            }
            $v = func_num_args() == 1 ? $objectOrValue : $value;
            Closure::bind(function ($c, $n, $v) { __phplang_reflect("static_set", $c, $n, $v); }, null, $this->class)($this->class, $this->name, $v);
            return;
        }
        Closure::bind(function ($o, $n, $v) { $o->$n = $v; }, null, $this->class)($objectOrValue, $this->name, $value);
    }
    public function isInitialized(?object $object = null): bool {
        if ($this->isStatic()) { return true; }
        return Closure::bind(function ($o, $n) { return isset($o->$n) || array_key_exists($n, get_object_vars($o)); }, null, $this->class)($object, $this->name);
    }
    public function getAttributes(?string $name = null, int $flags = 0): array { return ReflectionAttribute::__list("ReflectionProperty::getAttributes", ["prop", $this->class, $this->name], -1, Attribute::TARGET_PROPERTY, $name, $flags); }
}

class ReflectionClassConstant {
    const IS_PUBLIC = 1;
    const IS_PROTECTED = 2;
    const IS_PRIVATE = 4;
    const IS_FINAL = 32;
    public string $name = "";
    public string $class = "";
    public function __construct(object|string $class, string $constant) {
        $r = new ReflectionClass($class);
        if (!$r->hasConstant($constant)) { throw new ReflectionException("Constant " . $r->name . "::" . $constant . " does not exist"); }
        $this->name = $constant;
        $this->class = $r->name;
    }
    public function getName(): string { return $this->name; }
    public function getValue(): mixed { return constant($this->class . "::" . $this->name); }
    public function getDocComment(): string|false { return __phplang_reflect("doc", "const", $this->class, $this->name) ?: __phplang_reflect("doc", "case", $this->class, $this->name); }
    public function getAttributes(?string $name = null, int $flags = 0): array { return ReflectionAttribute::__list("ReflectionClassConstant::getAttributes", ["const", $this->class, $this->name], -1, Attribute::TARGET_CLASS_CONSTANT, $name, $flags); }
    public function getDeclaringClass(): ReflectionClass { return new ReflectionClass($this->class); }
}

class ReflectionClass {
    const IS_IMPLICIT_ABSTRACT = 16;
    const IS_EXPLICIT_ABSTRACT = 64;
    const IS_FINAL = 32;
    const IS_READONLY = 65536;
    public string $name = "";
    private static $__c = [];
    public function __construct(object|string $objectOrClass) {
        $info = __phplang_reflect("class", $objectOrClass);
        if ($info === false) { throw new ReflectionException("Class \"" . $objectOrClass . "\" does not exist"); }
        $this->name = $info["name"];
        $info["object"] = is_object($objectOrClass) ? $objectOrClass : null;
        ReflectionClass::$__c[spl_object_id($this)] = $info;
    }
    private function __info(): array { return ReflectionClass::$__c[spl_object_id($this)]; }
    public function getName(): string { return $this->name; }
    public function getShortName(): string { $p = strrpos($this->name, "\\"); return $p === false ? $this->name : substr($this->name, $p + 1); }
    public function getNamespaceName(): string { $p = strrpos($this->name, "\\"); return $p === false ? "" : substr($this->name, 0, $p); }
    public function inNamespace(): bool { return str_contains($this->name, "\\"); }
    public function isInterface(): bool { return $this->__info()["interface"]; }
    public function isTrait(): bool { return $this->__info()["trait"]; }
    public function isEnum(): bool { return $this->__info()["enum"]; }
    public function isAbstract(): bool { return $this->__info()["abstract"]; }
    public function isFinal(): bool { return $this->__info()["final"]; }
    public function isReadOnly(): bool { return false; }
    public function isInternal(): bool { return $this->__info()["internal"]; }
    public function isUserDefined(): bool { return !$this->__info()["internal"]; }
    public function isAnonymous(): bool { return $this->__info()["anonymous"]; }
    public function isCloneable(): bool { return $this->isInstantiable(); }
    public function isInstantiable(): bool {
        $i = $this->__info();
        if ($i["interface"] || $i["trait"] || $i["enum"] || $i["abstract"]) { return false; }
        $ctor = __phplang_reflect("method", $this->name, "__construct");
        return $ctor === null || ($ctor["modifiers"] & 1) != 0;
    }
    public function isIterable(): bool { return $this->implementsInterface("Traversable") && $this->isInstantiable(); }
    public function isIterateable(): bool { return $this->isIterable(); }
    public function getModifiers(): int {
        $i = $this->__info();
        return ($i["abstract"] && !$i["interface"] ? 64 : 0) | ($i["final"] ? 32 : 0);
    }
    public function getParentClass(): ReflectionClass|false {
        $p = $this->__info()["parent"];
        return $p === null ? false : new ReflectionClass($p);
    }
    public function getInterfaceNames(): array { return $this->__info()["interfaces"]; }
    public function getInterfaces(): array {
        $out = [];
        foreach ($this->getInterfaceNames() as $i) { $out[$i] = new ReflectionClass($i); }
        return $out;
    }
    public function getTraitNames(): array { return $this->__info()["traits"]; }
    public function getTraits(): array {
        $out = [];
        foreach ($this->getTraitNames() as $t) { $out[$t] = new ReflectionClass($t); }
        return $out;
    }
    public function implementsInterface(ReflectionClass|string $interface): bool {
        $name = $interface instanceof ReflectionClass ? $interface->name : $interface;
        $r = __phplang_reflect("class", $name);
        if ($r === false) { throw new ReflectionException("Interface \"" . $name . "\" does not exist"); }
        if (!$r["interface"]) { throw new ReflectionException($r["name"] . " is not an interface"); }
        return is_a($this->name, $r["name"], true);
    }
    public function isSubclassOf(ReflectionClass|string $class): bool {
        $name = $class instanceof ReflectionClass ? $class->name : $class;
        $r = __phplang_reflect("class", $name);
        if ($r === false) { throw new ReflectionException("Class \"" . $name . "\" does not exist"); }
        return strcasecmp($r["name"], $this->name) != 0 && is_a($this->name, $r["name"], true);
    }
    public function isInstance(object $object): bool { return is_a($object, $this->name); }
    public function hasMethod(string $name): bool { return __phplang_reflect("method", $this->name, $name) !== null; }
    public function getMethod(string $name): ReflectionMethod { return new ReflectionMethod($this->name, $name); }
    public function getMethods(?int $filter = null): array {
        $out = [];
        foreach (__phplang_reflect("methods", $this->name) as $m) {
            if ($filter === null || ($m["modifiers"] & $filter)) { $out[] = new ReflectionMethod($this->name, $m["name"]); }
        }
        return $out;
    }
    public function getConstructor(): ?ReflectionMethod { return $this->hasMethod("__construct") ? new ReflectionMethod($this->name, "__construct") : null; }
    // Made from an object, its dynamic properties count too.
    public function hasProperty(string $name): bool {
        $o = $this->__info()["object"];
        return __phplang_reflect("prop", $this->name, $name) !== null || ($o !== null && property_exists($o, $name));
    }
    public function getProperty(string $name): ReflectionProperty { return new ReflectionProperty($this->name, $name); }
    public function getProperties(?int $filter = null): array {
        $out = [];
        foreach (__phplang_reflect("props", $this->name) as $p) {
            if ($filter === null || ($p["modifiers"] & $filter)) { $out[] = new ReflectionProperty($this->name, $p["name"]); }
        }
        return $out;
    }
    // The static properties first, then the instance ones.
    public function getDefaultProperties(): array {
        $out = [];
        foreach ([true, false] as $static) {
            foreach (__phplang_reflect("props", $this->name) as $p) {
                if ($p["default"] && (($p["modifiers"] & 16) != 0) === $static) {
                    $out[$p["name"]] = __phplang_reflect("prop_default", $this->name, $p["name"]);
                }
            }
        }
        return $out;
    }
    public function getStaticProperties(): array {
        $out = [];
        foreach ($this->getProperties(16) as $p) { $out[$p->name] = $p->getValue(); }
        return $out;
    }
    public function getStaticPropertyValue(string $name, mixed $default = null): mixed {
        if (!$this->hasProperty($name)) {
            if (func_num_args() > 1) { return $default; }
            throw new ReflectionException("Property " . $this->name . "::\$" . $name . " does not exist");
        }
        return $this->getProperty($name)->getValue();
    }
    public function setStaticPropertyValue(string $name, mixed $value): void { $this->getProperty($name)->setValue(null, $value); }
    public function hasConstant(string $name): bool { return array_key_exists($name, __phplang_reflect("consts", $this->name)); }
    public function getConstants(?int $filter = null): array { return __phplang_reflect("consts", $this->name); }
    public function getConstant(string $name): mixed {
        $all = __phplang_reflect("consts", $this->name);
        if (!array_key_exists($name, $all)) {
            __phplang_reflect("deprecated", "ReflectionClass::getConstant() for a non-existent constant is deprecated, use ReflectionClass::hasConstant() to check if the constant exists");
            return false;
        }
        return $all[$name];
    }
    public function getReflectionConstant(string $name): ReflectionClassConstant|false { return $this->hasConstant($name) ? new ReflectionClassConstant($this->name, $name) : false; }
    public function newInstance(mixed ...$args): object { $c = $this->name; return new $c(...$args); }
    public function newInstanceArgs(array $args = []): ?object { $c = $this->name; return new $c(...$args); }
    public function newInstanceWithoutConstructor(): object { return __phplang_reflect("instantiate", $this->name); }
    public function getFileName(): string|false { return false; }
    public function getDocComment(): string|false { return __phplang_reflect("doc", "class", $this->name, ""); }
    public function getAttributes(?string $name = null, int $flags = 0): array { return ReflectionAttribute::__list("ReflectionClass::getAttributes", ["class", $this->name, ""], -1, Attribute::TARGET_CLASS, $name, $flags); }
    public function getExtensionName(): string|false { return false; }
}

class ReflectionObject extends ReflectionClass {
    public function __construct(object $object) { parent::__construct($object); }
}

class ReflectionEnum extends ReflectionClass {
    public function __construct(object|string $objectOrClass) {
        parent::__construct($objectOrClass);
        if (!$this->isEnum()) { throw new ReflectionException("Class \"" . $this->name . "\" is not an enum"); }
    }
    public function isBacked(): bool { return $this->implementsInterface("BackedEnum"); }
    public function getCases(): array {
        $c = $this->name;
        $kind = $this->isBacked() ? ReflectionEnumBackedCase::class : ReflectionEnumUnitCase::class;
        $out = [];
        foreach ($c::cases() as $case) { $out[] = new $kind($c, $case->name); }
        return $out;
    }
    public function hasCase(string $name): bool { foreach ($this->name::cases() as $case) { if ($case->name === $name) { return true; } } return false; }
    public function getCase(string $name): ReflectionEnumUnitCase {
        if (!$this->hasConstant($name)) { throw new ReflectionException("Case " . $this->name . "::" . $name . " does not exist"); }
        if (!$this->hasCase($name)) { throw new ReflectionException($this->name . "::" . $name . " is not a case"); }
        return $this->isBacked() ? new ReflectionEnumBackedCase($this->name, $name) : new ReflectionEnumUnitCase($this->name, $name);
    }
}

class ReflectionEnumUnitCase extends ReflectionClassConstant {
    public function __construct(object|string $class, string $constant) {
        parent::__construct($class, $constant);
        if (!(constant($this->class . "::" . $constant) instanceof UnitEnum)) { throw new ReflectionException("Constant " . $this->class . "::" . $constant . " is not a case"); }
    }
    public function getEnum(): ReflectionEnum { return new ReflectionEnum($this->class); }
    public function getValue(): UnitEnum { return constant($this->class . "::" . $this->name); }
}

class ReflectionEnumBackedCase extends ReflectionEnumUnitCase {
    public function __construct(object|string $class, string $constant) {
        parent::__construct($class, $constant);
        if (!($this->getValue() instanceof BackedEnum)) { throw new ReflectionException("Enum case " . $this->class . "::" . $constant . " is not a backed case"); }
    }
    public function getBackingValue(): int|string { return $this->getValue()->value; }
}
"##;

/// The SPL data-structure classes, written in PHP. They store elements in an
/// internal array property and mutate it through `$this`, which is the only way
/// a PHP array — a value, not a handle — can be mutated in place. Aliasing it to
/// a local first would copy it, and the write would be lost. Method-driven
/// (offsetGet/push/…), with `getIterator` returning the backing array.
const SPL_PRELUDE: &str = r#"
class stdClass {}
// `unserialize` restores an object of an unknown class into this placeholder,
// carrying the original name in `__PHP_Incomplete_Class_Name`.
class __PHP_Incomplete_Class {}
// SplDoublyLinkedList, SplQueue and SplStack: a port of ext/spl/spl_dllist.c.
// The list is a PHP list kept head first in `$dllist`, beside the mode bits a
// program set in `$flags`; `__debugInfo` reports both under the names, and in
// the order, the reference's debug view gives them. SplStack's LIFO bit and
// both subclasses' frozen-mode bit belong to the class, as the reference's
// `create_object` sets them. The traverse cursor — the index of the element it
// stands on, or null, and its position — lives in a static table keyed by
// object id, so it shows nowhere.
class SplDoublyLinkedList implements Iterator, Countable, ArrayAccess, Serializable {
    const IT_MODE_LIFO = 2;
    const IT_MODE_FIFO = 0;
    const IT_MODE_DELETE = 1;
    const IT_MODE_KEEP = 0;
    private $flags = 0;
    private $dllist = [];
    private static $__cursor = [];
    private function __flags() {
        if ($this instanceof SplStack) { return ($this->flags & 1) | 6; }
        if ($this instanceof SplQueue) { return ($this->flags & 1) | 4; }
        return $this->flags;
    }
    // `spl_ptr_llist_offset`: the list index of the element `$index` steps
    // from the head, or from the tail in LIFO mode.
    private function __at($index) {
        return ($this->__flags() & 2) ? count($this->dllist) - 1 - $index : $index;
    }
    private function __cur() { return self::$__cursor[spl_object_id($this)] ?? [null, 0]; }
    private function __setCur($ptr, $pos) { self::$__cursor[spl_object_id($this)] = [$ptr, $pos]; }
    // Keep the cursor on its element across an insertion (`$delta` 1) or a
    // removal (`$delta` -1) at list index `$at`; removing its element clears it.
    private function __moved($at, $delta) {
        [$ptr, $pos] = $this->__cur();
        if ($ptr === null) { return; }
        if ($delta < 0 && $ptr === $at) { $ptr = null; } elseif ($ptr >= $at) { $ptr += $delta; }
        $this->__setCur($ptr, $pos);
    }
    private function __outOfRange($method) {
        throw new OutOfRangeException("SplDoublyLinkedList::$method(): Argument #1 (\$index) is out of range");
    }
    public function add(int $index, mixed $value): void {
        $n = count($this->dllist);
        if ($index < 0 || $index > $n) { $this->__outOfRange("add"); }
        if ($index === $n) { $this->dllist[] = $value; return; }
        $at = $this->__at($index);
        array_splice($this->dllist, $at, 0, [$value]);
        $this->__moved($at, 1);
    }
    public function pop(): mixed {
        if (count($this->dllist) === 0) { throw new RuntimeException("Can't pop from an empty datastructure"); }
        $v = array_pop($this->dllist);
        $this->__moved(count($this->dllist), -1);
        return $v;
    }
    public function shift(): mixed {
        if (count($this->dllist) === 0) { throw new RuntimeException("Can't shift from an empty datastructure"); }
        $v = array_shift($this->dllist);
        $this->__moved(0, -1);
        return $v;
    }
    public function push(mixed $value): void { $this->dllist[] = $value; }
    public function unshift(mixed $value): void { array_unshift($this->dllist, $value); $this->__moved(0, 1); }
    public function top(): mixed {
        if (count($this->dllist) === 0) { throw new RuntimeException("Can't peek at an empty datastructure"); }
        return $this->dllist[count($this->dllist) - 1];
    }
    public function bottom(): mixed {
        if (count($this->dllist) === 0) { throw new RuntimeException("Can't peek at an empty datastructure"); }
        return $this->dllist[0];
    }
    public function count(): int { return count($this->dllist); }
    public function isEmpty(): bool { return count($this->dllist) === 0; }
    public function setIteratorMode(int $mode): int {
        $f = $this->__flags();
        if (($f & 4) && ($f & 2) !== ($mode & 2)) {
            throw new RuntimeException("Iterators' LIFO/FIFO modes for SplStack/SplQueue objects are frozen");
        }
        $this->flags = $mode & 3;
        return $this->__flags();
    }
    public function getIteratorMode(): int { return $this->__flags(); }
    public function offsetExists(int $index): bool { return $index >= 0 && $index < count($this->dllist); }
    public function offsetGet(int $index): mixed {
        if ($index < 0 || $index >= count($this->dllist)) { $this->__outOfRange("offsetGet"); }
        return $this->dllist[$this->__at($index)];
    }
    public function offsetSet(?int $index, mixed $value): void {
        if ($index === null) { $this->dllist[] = $value; return; }
        if ($index < 0 || $index >= count($this->dllist)) { $this->__outOfRange("offsetSet"); }
        $this->dllist[$this->__at($index)] = $value;
    }
    public function offsetUnset(int $index): void {
        if ($index < 0 || $index >= count($this->dllist)) { $this->__outOfRange("offsetUnset"); }
        $at = $this->__at($index);
        array_splice($this->dllist, $at, 1);
        $this->__moved($at, -1);
    }
    public function rewind(): void {
        $n = count($this->dllist);
        if ($this->__flags() & 2) {
            $this->__setCur($n > 0 ? $n - 1 : null, $n - 1);
        } else {
            $this->__setCur($n > 0 ? 0 : null, 0);
        }
    }
    public function valid(): bool { return $this->__cur()[0] !== null; }
    public function current(): mixed {
        $ptr = $this->__cur()[0];
        return $ptr === null ? null : $this->dllist[$ptr];
    }
    public function key(): int { return $this->__cur()[1]; }
    public function prev(): void { $this->__forward($this->__flags() ^ 2); }
    public function next(): void { $this->__forward($this->__flags()); }
    // `spl_dllist_it_helper_move_forward`: step toward the tail (FIFO) or the
    // head (LIFO), removing the element left behind in DELETE mode.
    private function __forward($flags) {
        [$ptr, $pos] = $this->__cur();
        if ($ptr === null) { return; }
        if ($flags & 2) {
            $ptr--;
            $pos--;
            if ($flags & 1) { array_pop($this->dllist); }
        } else {
            $ptr++;
            if ($flags & 1) { array_shift($this->dllist); $ptr--; } else { $pos++; }
        }
        $this->__setCur($ptr >= 0 && $ptr < count($this->dllist) ? $ptr : null, $pos);
    }
    // The properties a program gave the object, as `zend_std_get_properties`
    // holds them — everything but the list's own two slots.
    private function __members() {
        $m = (array) $this;
        unset($m["\0SplDoublyLinkedList\0flags"], $m["\0SplDoublyLinkedList\0dllist"]);
        return $m;
    }
    public function serialize(): string {
        $s = serialize($this->__flags());
        foreach ($this->dllist as $v) { $s .= ":" . serialize($v); }
        return $s;
    }
    public function unserialize(string $data): void {}
    public function __serialize(): array { return [$this->__flags(), $this->dllist, $this->__members()]; }
    public function __unserialize(array $data): void {
        if (!isset($data[0], $data[1], $data[2]) || !is_int($data[0]) || !is_array($data[1]) || !is_array($data[2])) {
            throw new UnexpectedValueException("Incomplete or ill-typed serialization data");
        }
        $this->flags = $data[0] & 3;
        foreach ($data[1] as $v) { $this->dllist[] = $v; }
        foreach ($data[2] as $k => $v) { $this->$k = $v; }
    }
    public function __debugInfo(): array {
        $m = $this->__members();
        $m["\0SplDoublyLinkedList\0flags"] = $this->__flags();
        $m["\0SplDoublyLinkedList\0dllist"] = $this->dllist;
        return $m;
    }
}
class SplQueue extends SplDoublyLinkedList {
    public function enqueue(mixed $value): void { $this->push($value); }
    public function dequeue(): mixed { return $this->shift(); }
}
class SplStack extends SplDoublyLinkedList {}
// SplFixedArray: a port of ext/spl/spl_fixedarray.c. The elements are a list
// in `$__elements`; `__debugInfo` shows them as the reference's
// `get_properties_for` does, as integer-keyed entries ahead of any property.
class SplFixedArray implements IteratorAggregate, ArrayAccess, Countable, JsonSerializable {
    private $__elements = [];
    public function __construct(int $size = 0) {
        if ($size < 0) {
            throw new ValueError('SplFixedArray::__construct(): Argument #1 ($size) must be greater than or equal to 0');
        }
        // A second __construct() call leaves a non-empty array alone.
        if (count($this->__elements) > 0) { return; }
        $this->__elements = $size > 0 ? array_fill(0, $size, null) : [];
    }
    // The element index `$index` names: `Index invalid or out of range` past
    // either end, and the offset conversion's TypeError for a non-integer.
    private function __index($index) {
        $i = __phplang_spl_offset($index);
        if ($i < 0 || $i >= count($this->__elements)) {
            throw new OutOfBoundsException("Index invalid or out of range");
        }
        return $i;
    }
    public function count(): int { return count($this->__elements); }
    public function toArray(): array { return $this->__elements; }
    public static function fromArray(array $array, bool $preserveKeys = true): SplFixedArray {
        $fa = new SplFixedArray();
        if (count($array) > 0 && $preserveKeys) {
            $max = 0;
            foreach ($array as $k => $v) {
                if (!is_int($k) || $k < 0) {
                    throw new InvalidArgumentException("array must contain only positive integer keys");
                }
                if ($k > $max) { $max = $k; }
            }
            $fa->__elements = array_fill(0, $max + 1, null);
            foreach ($array as $k => $v) { $fa->__elements[$k] = $v; }
        } elseif (count($array) > 0) {
            $fa->__elements = array_values($array);
        }
        return $fa;
    }
    public function getSize(): int { return count($this->__elements); }
    public function setSize(int $size): true {
        if ($size < 0) {
            throw new ValueError('SplFixedArray::setSize(): Argument #1 ($size) must be greater than or equal to 0');
        }
        $n = count($this->__elements);
        if ($size < $n) {
            $this->__elements = array_slice($this->__elements, 0, $size);
        } else {
            for ($i = $n; $i < $size; $i++) { $this->__elements[] = null; }
        }
        return true;
    }
    public function offsetExists($index): bool {
        $i = __phplang_spl_offset($index);
        return $i >= 0 && $i < count($this->__elements) && $this->__elements[$i] !== null;
    }
    public function offsetGet($index): mixed { return $this->__elements[$this->__index($index)]; }
    public function offsetSet($index, mixed $value): void { $this->__elements[$this->__index($index)] = $value; }
    public function offsetUnset($index): void { $this->__elements[$this->__index($index)] = null; }
    public function getIterator(): Iterator { return new ArrayIterator($this->__elements); }
    public function jsonSerialize(): array { return $this->__elements; }
    private function __members() {
        $m = (array) $this;
        unset($m["\0SplFixedArray\0__elements"]);
        return $m;
    }
    public function __serialize(): array {
        $out = $this->__elements;
        foreach ($this->__members() as $k => $v) { $out[$k] = $v; }
        return $out;
    }
    public function __unserialize(array $data): void {
        if (count($this->__elements) > 0) { return; }
        foreach ($data as $k => $v) {
            if (is_int($k)) { $this->__elements[] = $v; } else { $this->$k = $v; }
        }
    }
    public function __debugInfo(): array {
        $out = $this->__elements;
        foreach ($this->__members() as $k => $v) { $out[$k] = $v; }
        return $out;
    }
}
// ext/spl/spl_array.c's ArrayObject. The flags and the iterator class live in
// a static table keyed by object id, as ArrayIterator's flags do, so the
// instance shows only its storage. An object passed as the backing array is
// copied into it: the reference's write-through to that object is not
// modelled (BUGS.md).
class ArrayObject implements IteratorAggregate, ArrayAccess, Serializable, Countable {
    const STD_PROP_LIST = 1;
    const ARRAY_AS_PROPS = 2;
    private $storage = [];
    // id => [the flags a program set, the iterator class]
    private static $__state = [];
    public function __construct(array|object $array = [], int $flags = 0, string $iteratorClass = ArrayIterator::class) {
        $iteratorClass = self::__itclass($iteratorClass, 'ArrayObject::__construct(): Argument #3');
        if (func_num_args() == 0) { return; }
        $this->storage = self::__backing($array, 'ArrayObject::__construct()');
        self::$__state[spl_object_id($this)] = [$flags & ~0xFFFF0000, $iteratorClass];
    }
    // `spl_array_set_array`: an object backing is deprecated since 8.5. The
    // object itself is the storage — its properties are the elements, or,
    // for another ArrayObject / ArrayIterator, ITS storage — so a write
    // through either reaches both.
    private static function __backing($array, $fn) {
        if (is_array($array)) { return $array; }
        __phplang_deprecated("$fn: Using an object as a backing array for ArrayObject is deprecated, as it allows violating class constraints and invariants");
        return $array;
    }
    // Whether the storage is another ArrayObject / ArrayIterator, whose own
    // element methods the operations forward to.
    private function __shared() { return $this->storage instanceof ArrayObject || $this->storage instanceof ArrayIterator; }
    // The `C` parameter type: a class derived from ArrayIterator.
    private static function __itclass($class, $arg) {
        if (!class_exists($class) || !is_a($class, ArrayIterator::class, true)) {
            throw new TypeError("$arg (\$iteratorClass) must be a class name derived from ArrayIterator, $class given");
        }
        if (strcasecmp($class, ArrayIterator::class) === 0) { return ArrayIterator::class; }
        foreach (get_declared_classes() as $c) { if (strcasecmp($c, $class) === 0) { return $c; } }
        return $class;
    }
    private function __state() { return self::$__state[spl_object_id($this)] ?? [0, ArrayIterator::class]; }
    public function offsetGet(mixed $key): mixed {
        if (!is_object($this->storage)) { return $this->storage[$key]; }
        if ($this->__shared()) { return $this->storage->offsetGet($key); }
        return $this->storage->$key;
    }
    public function offsetSet(mixed $key, mixed $value): void {
        if (!is_object($this->storage)) {
            if ($key === null) { $this->storage[] = $value; } else { $this->storage[$key] = $value; }
            return;
        }
        if ($this->__shared()) { $this->storage->offsetSet($key, $value); return; }
        // An append takes the next INTEGER key of the property table; a named
        // element is the string-named property.
        if ($key === null) { __phplang_obj_append($this->storage, $value); return; }
        $this->storage->$key = $value;
    }
    public function offsetExists(mixed $key): bool {
        if (!is_object($this->storage)) { return isset($this->storage[$key]); }
        if ($this->__shared()) { return $this->storage->offsetExists($key); }
        return isset($this->storage->$key);
    }
    public function offsetUnset(mixed $key): void {
        if (!is_object($this->storage)) { unset($this->storage[$key]); return; }
        if ($this->__shared()) { $this->storage->offsetUnset($key); return; }
        unset($this->storage->$key);
    }
    // ARRAY_AS_PROPS: a property the object does not have is an element
    // (`spl_array_read_property` and its siblings). The host calls these only
    // while the flag is set.
    public function __get($name) { return $this->offsetGet($name); }
    public function __set($name, $value) { $this->offsetSet($name, $value); }
    public function __isset($name) { return $this->offsetExists($name) && $this->offsetGet($name) !== null; }
    public function __unset($name) { $this->offsetUnset($name); }
    public function append(mixed $value): void {
        if (!is_object($this->storage)) { $this->storage[] = $value; return; }
        if ($this->__shared()) { $this->storage->append($value); return; }
        throw new Error("Cannot append properties to objects, use ArrayObject::offsetSet() instead");
    }
    public function count(): int { return count($this->__elements(false)); }
    // An object's elements are its whole property table, as `(array)` reads
    // it; counting and iterating skip the non-public properties.
    public function getArrayCopy(): array { return $this->__elements(true); }
    public function __elements(bool $all) {
        if (!is_object($this->storage)) { return $this->storage; }
        if ($this->__shared()) { return $this->storage->__elements($all); }
        $t = (array) $this->storage;
        if (!$all) { foreach ($t as $k => $_) { if (is_string($k) && $k !== "" && $k[0] === "\0") { unset($t[$k]); } } }
        return $t;
    }
    public function exchangeArray(array|object $array): array { $old = $this->getArrayCopy(); $this->storage = self::__backing($array, 'ArrayObject::exchangeArray()'); return $old; }
    // `spl_array_object_new_ex` for the iterator: it starts with this object's flags.
    public function getIterator(): Iterator { [$flags, $class] = $this->__state(); return ArrayIterator::__over($this, $flags, $class); }
    public function getFlags(): int { return $this->__state()[0]; }
    public function setFlags(int $flags): void { self::$__state[spl_object_id($this)] = [$flags & ~0xFFFF0000, $this->__state()[1]]; }
    public function getIteratorClass(): string { return $this->__state()[1]; }
    public function setIteratorClass(string $iteratorClass): void { self::$__state[spl_object_id($this)] = [$this->__state()[0], self::__itclass($iteratorClass, 'ArrayObject::setIteratorClass(): Argument #1')]; }
    // `spl_array_object_new_ex` with `clone_orig`: the copy keeps the flags and the iterator class.
    private function __phplang_clone($orig) { self::$__state[spl_object_id($this)] = $orig->__state(); }
    public function serialize(): string { return __SplArraySerial::format($this->__state()[0], $this->storage, array_diff_key(get_object_vars($this), ['storage' => 1])); }
    public function unserialize(string $data): void {
        $parsed = __SplArraySerial::parse($data);
        if ($parsed === null) { return; }
        $flags = $parsed[0]; $storage = $parsed[1]; $members = $parsed[2];
        $this->storage = $storage;
        foreach ($members as $k => $v) { $this->$k = $v; }
        self::$__state[spl_object_id($this)] = [$flags, $this->__state()[1]];
    }
    public function __serialize(): array { [$flags, $class] = $this->__state(); return [$flags, $this->storage, array_diff_key(get_object_vars($this), ['storage' => 1]), $class === ArrayIterator::class ? null : $class]; }
    public function __unserialize(array $data): void { $this->storage = $data[1]; foreach ($data[2] ?? [] as $k => $v) { $this->$k = $v; } self::$__state[spl_object_id($this)] = [$data[0], $data[3] ?? ArrayIterator::class]; }
    public function asort(int $flags = SORT_REGULAR): bool { return $this->__sort("asort", $flags); }
    public function ksort(int $flags = SORT_REGULAR): bool { return $this->__sort("ksort", $flags); }
    public function uasort(callable $callback): bool { return $this->__sort("uasort", $callback); }
    public function uksort(callable $callback): bool { return $this->__sort("uksort", $callback); }
    public function natsort(): bool { return $this->__sort("natsort"); }
    public function natcasesort(): bool { return $this->__sort("natcasesort"); }
    // Sort the storage in place; an object backing has its properties put
    // back in the sorted order, a shared one is sorted by its owner.
    private function __sort($fn, ...$args) {
        if (!is_object($this->storage)) { return $fn($this->storage, ...$args); }
        if ($this->__shared()) { return $this->storage->$fn(...$args); }
        $table = (array) $this->storage;
        $r = $fn($table, ...$args);
        __phplang_obj_replace($this->storage, $table);
        return $r;
    }
}
interface SeekableIterator extends Iterator {
    public function seek(int $offset): void;
}
interface Serializable {
    public function serialize();
    public function unserialize(string $data);
}
interface SplObserver {
    public function update(SplSubject $subject): void;
}
interface SplSubject {
    public function attach(SplObserver $observer): void;
    public function detach(SplObserver $observer): void;
    public function notify(): void;
}
interface OuterIterator extends Iterator {
    public function getInnerIterator(): ?Iterator;
}
interface RecursiveIterator extends Iterator {
    public function hasChildren(): bool;
    public function getChildren(): ?RecursiveIterator;
}
#[Attribute(Attribute::TARGET_CLASS)]
final class Attribute {
    const TARGET_CLASS = 1;
    const TARGET_FUNCTION = 2;
    const TARGET_METHOD = 4;
    const TARGET_PROPERTY = 8;
    const TARGET_CLASS_CONSTANT = 16;
    const TARGET_PARAMETER = 32;
    const TARGET_CONSTANT = 64;
    const TARGET_ALL = 127;
    const IS_REPEATABLE = 128;
    public int $flags;
    public function __construct(int $flags = Attribute::TARGET_ALL) { $this->flags = $flags; }
}
#[Attribute(Attribute::TARGET_METHOD)]
final class ReturnTypeWillChange { public function __construct() {} }
#[Attribute(Attribute::TARGET_CLASS)]
final class AllowDynamicProperties { public function __construct() {} }
#[Attribute(Attribute::TARGET_PARAMETER)]
final class SensitiveParameter { public function __construct() {} }
#[Attribute(Attribute::TARGET_METHOD | Attribute::TARGET_PROPERTY)]
final class Override { public function __construct() {} }
#[Attribute(Attribute::TARGET_CLASS | Attribute::TARGET_FUNCTION | Attribute::TARGET_METHOD | Attribute::TARGET_CLASS_CONSTANT | Attribute::TARGET_CONSTANT)]
final class Deprecated {
    public readonly ?string $message;
    public readonly ?string $since;
    public function __construct(?string $message = null, ?string $since = null) { $this->message = $message; $this->since = $since; }
}
#[Attribute(Attribute::TARGET_FUNCTION | Attribute::TARGET_METHOD)]
final class NoDiscard {
    public readonly ?string $message;
    public function __construct(?string $message = null) { $this->message = $message; }
}
// `ReflectionAttribute`: one attribute of a declaration, read through
// `__phplang_reflect("attrs" / "attr_args")` on the declaration's
// `[kind, target, member]` spec (a parameter's position, else -1).
final class ReflectionAttribute {
    const IS_INSTANCEOF = 2;
    public string $name = "";
    // id => [kind, target, member, parameter position, index, target flag, repeated]
    private static $__a = [];
    // The `getAttributes($name, $flags)` of the declaration `$spec`, which
    // the reference names `$fn` in its argument errors.
    public static function __list(string $fn, array $spec, int $pos, int $target, ?string $name, int $flags): array {
        if ($flags & ~ReflectionAttribute::IS_INSTANCEOF) { throw new ValueError("$fn(): Argument #2 (\$flags) must be a valid attribute filter flag"); }
        if ($name !== null && ($flags & ReflectionAttribute::IS_INSTANCEOF) && !class_exists($name) && !interface_exists($name)) { throw new Error("Class \"$name\" not found"); }
        [$k, $t, $n] = $spec;
        $names = __phplang_reflect("attrs", $k, $t, $n, $pos);
        $counts = [];
        foreach ($names as $a) { $counts[strtolower($a)] = ($counts[strtolower($a)] ?? 0) + 1; }
        $out = [];
        foreach ($names as $i => $a) {
            if ($name !== null && ($flags & ReflectionAttribute::IS_INSTANCEOF ? !is_a($a, $name, true) : strcasecmp(ltrim($name, "\\"), $a) !== 0)) { continue; }
            $r = new ReflectionAttribute();
            $r->name = $a;
            ReflectionAttribute::$__a[spl_object_id($r)] = [$k, $t, $n, $pos, $i, $target, $counts[strtolower($a)] > 1];
            $out[] = $r;
        }
        return $out;
    }
    public function getName(): string { return $this->name; }
    public function getTarget(): int { return ReflectionAttribute::$__a[spl_object_id($this)][5]; }
    public function isRepeated(): bool { return ReflectionAttribute::$__a[spl_object_id($this)][6]; }
    public function getArguments(): array {
        [$k, $t, $n, $p, $i] = ReflectionAttribute::$__a[spl_object_id($this)];
        return __phplang_reflect("attr_args", $k, $t, $n, $p, $i);
    }
    // `zend_get_attribute_object`: the class must exist and be an attribute
    // whose `#[Attribute]` flags admit this target, and repeat only when
    // `IS_REPEATABLE`.
    public function newInstance(): object {
        $c = $this->name;
        if (!class_exists($c)) { throw new Error("Attribute class \"$c\" not found"); }
        $meta = (new ReflectionClass($c))->getAttributes("Attribute");
        if (!$meta) { throw new Error("Attempting to use non-attribute class \"$c\" as attribute"); }
        $args = $meta[0]->getArguments();
        $allowed = $args[0] ?? $args["flags"] ?? Attribute::TARGET_ALL;
        $target = $this->getTarget();
        if (!($allowed & $target)) {
            $names = ["class", "function", "method", "property", "class constant", "parameter", "constant"];
            $shown = [];
            foreach ($names as $bit => $label) { if ($allowed & (1 << $bit)) { $shown[] = $label; } }
            throw new Error("Attribute \"$c\" cannot target " . $names[(int) log($target, 2)] . " (allowed targets: " . implode(", ", $shown) . ")");
        }
        if ($this->isRepeated() && !($allowed & Attribute::IS_REPEATABLE)) { throw new Error("Attribute \"$c\" must not be repeated"); }
        return new $c(...$this->getArguments());
    }
}
// An iterator over an array, positioned by a cursor kept outside the
// instance (a static table keyed by object id) so the instance shows only
// its storage, as the reference's does.
class ArrayIterator implements SeekableIterator, ArrayAccess, Serializable, Countable {
    const STD_PROP_LIST = 1;
    const ARRAY_AS_PROPS = 2;
    private $storage = [];
    private static $__cursor = [];
    // id => the flags a program set, `ar_flags & ~SPL_ARRAY_INT_MASK`.
    private static $__flags = [];
    // An object backing is the storage itself, as ArrayObject's: its property
    // table, or another ArrayObject / ArrayIterator's storage, which a write
    // through either reaches.
    public function __construct(array|object $array = [], int $flags = 0) {
        if (!is_array($array)) { __phplang_deprecated("ArrayIterator::__construct(): Using an object as a backing array for ArrayIterator is deprecated, as it allows violating class constraints and invariants"); }
        $this->storage = $array;
        self::$__flags[spl_object_id($this)] = $flags & ~0xFFFF0000;
    }
    private function __shared() { return $this->storage instanceof ArrayObject || $this->storage instanceof ArrayIterator; }
    // The elements as they stand: the array, or the backing object's table
    // — all of it for a copy, and for counting and iterating without the
    // non-public properties, which `spl_array_skip_protected` steps over.
    private function __table(bool $all = false) {
        if (!is_object($this->storage)) { return $this->storage; }
        if ($this->__shared()) { return $this->storage->__elements($all); }
        $t = (array) $this->storage;
        if (!$all) { foreach ($t as $k => $_) { if (is_string($k) && $k !== "" && $k[0] === "\0") { unset($t[$k]); } } }
        return $t;
    }
    public function __elements(bool $all) { return $this->__table($all); }
    // `ArrayObject::getIterator`: an iterator of `$class` sharing the
    // object's storage, made without running a constructor.
    public static function __over(ArrayObject $owner, int $flags, string $class) {
        $it = __phplang_reflect("instantiate", $class);
        $it->storage = $owner;
        self::$__flags[spl_object_id($it)] = $flags;
        return $it;
    }
    private function __at() {
        $c = self::$__cursor[spl_object_id($this)] ?? [0, null];
        if ($c[1] === null) { $c[1] = array_keys($this->__table()); self::$__cursor[spl_object_id($this)] = $c; }
        return $c;
    }
    private function __moved() { self::$__cursor[spl_object_id($this)][1] = null; }
    public function current(): mixed { [$i, $keys] = $this->__at(); return $i < count($keys) ? $this->__table()[$keys[$i]] : null; }
    public function key(): string|int|null { [$i, $keys] = $this->__at(); return $keys[$i] ?? null; }
    public function next(): void { [$i, $keys] = $this->__at(); self::$__cursor[spl_object_id($this)] = [$i + 1, $keys]; }
    public function rewind(): void { self::$__cursor[spl_object_id($this)] = [0, array_keys($this->__table())]; }
    public function valid(): bool { [$i, $keys] = $this->__at(); return $i < count($keys); }
    public function seek(int $offset): void {
        $keys = array_keys($this->__table());
        if ($offset < 0 || $offset >= count($keys)) { throw new OutOfBoundsException("Seek position $offset is out of range"); }
        self::$__cursor[spl_object_id($this)] = [$offset, $keys];
    }
    public function offsetGet(mixed $key): mixed {
        if (!is_object($this->storage)) { return $this->storage[$key]; }
        if ($this->__shared()) { return $this->storage->offsetGet($key); }
        return $this->storage->$key;
    }
    public function offsetSet(mixed $key, mixed $value): void {
        if (!is_object($this->storage)) {
            if ($key === null) { $this->storage[] = $value; } else { $this->storage[$key] = $value; }
        } elseif ($this->__shared()) {
            $this->storage->offsetSet($key, $value);
        } elseif ($key === null) {
            __phplang_obj_append($this->storage, $value);
        } else {
            $this->storage->$key = $value;
        }
        $this->__moved();
    }
    public function offsetExists(mixed $key): bool {
        if (!is_object($this->storage)) { return isset($this->storage[$key]); }
        if ($this->__shared()) { return $this->storage->offsetExists($key); }
        return isset($this->storage->$key);
    }
    public function offsetUnset(mixed $key): void {
        if (!is_object($this->storage)) { unset($this->storage[$key]); }
        elseif ($this->__shared()) { $this->storage->offsetUnset($key); }
        else { unset($this->storage->$key); }
        $this->__moved();
    }
    // ARRAY_AS_PROPS, as ArrayObject's.
    public function __get($name) { return $this->offsetGet($name); }
    public function __set($name, $value) { $this->offsetSet($name, $value); }
    public function __isset($name) { return $this->offsetExists($name) && $this->offsetGet($name) !== null; }
    public function __unset($name) { $this->offsetUnset($name); }
    public function append(mixed $value): void {
        if (is_object($this->storage) && !$this->__shared()) { throw new Error("Cannot append properties to objects, use ArrayIterator::offsetSet() instead"); }
        $this->offsetSet(null, $value);
    }
    public function count(): int { return count($this->__table()); }
    public function getArrayCopy(): array { return $this->__table(true); }
    public function getFlags(): int { return self::$__flags[spl_object_id($this)] ?? 0; }
    public function setFlags(int $flags): void { self::$__flags[spl_object_id($this)] = $flags & ~0xFFFF0000; }
    // `spl_array_object_new_ex` with `clone_orig`: the copy keeps the flags; its
    // position starts over.
    private function __phplang_clone($orig) { self::$__flags[spl_object_id($this)] = self::$__flags[spl_object_id($orig)] ?? 0; }
    public function serialize(): string { return __SplArraySerial::format(self::$__flags[spl_object_id($this)] ?? 0, $this->storage, array_diff_key(get_object_vars($this), ['storage' => 1])); }
    public function unserialize(string $data): void {
        $parsed = __SplArraySerial::parse($data);
        if ($parsed === null) { return; }
        $flags = $parsed[0]; $storage = $parsed[1]; $members = $parsed[2];
        $this->storage = $storage;
        foreach ($members as $k => $v) { $this->$k = $v; }
        self::$__flags[spl_object_id($this)] = $flags;
    }
    public function __serialize(): array { return [self::$__flags[spl_object_id($this)] ?? 0, $this->storage, array_diff_key(get_object_vars($this), ['storage' => 1]), null]; }
    public function __unserialize(array $data): void { $this->storage = $data[1]; foreach ($data[2] ?? [] as $k => $v) { $this->$k = $v; } self::$__flags[spl_object_id($this)] = $data[0]; }
    public function asort(int $flags = SORT_REGULAR): bool { return asort($this->storage, $flags); }
    public function ksort(int $flags = SORT_REGULAR): bool { return ksort($this->storage, $flags); }
    public function uasort(callable $callback): bool { return uasort($this->storage, $callback); }
    public function uksort(callable $callback): bool { return uksort($this->storage, $callback); }
    public function natsort(): bool { return natsort($this->storage); }
    public function natcasesort(): bool { return natcasesort($this->storage); }
}
// SplObjectStorage: a port of ext/spl/spl_observer.c. `$storage` is the
// reference's hash table in insertion order — a key per object, its handle
// unless a subclass overrides getHash(), whose string is the key then — of
// [object, info] pairs. `$pos` is the table's internal pointer (an offset into
// that order; the count is past the end) and `$index` the iteration index.
class SplObjectStorage implements Countable, SeekableIterator, Serializable, ArrayAccess {
    private $storage = [];
    private $pos = 0;
    private $index = 0;
    // `spl_object_storage_get_hash`
    private function __key($object) {
        if (__phplang_method_owner($this, 'gethash') === 'splobjectstorage') {
            return spl_object_id($object);
        }
        $h = $this->getHash($object);
        if (!is_string($h)) {
            $given = is_bool($h) ? ($h ? "true" : "false") : get_debug_type($h);
            throw new TypeError(get_class($this) . "::getHash(): Return value must be of type string, $given returned");
        }
        return $h;
    }
    private function __object($object, $method) {
        if (!is_object($object)) {
            throw new TypeError("SplObjectStorage::$method(): Argument #1 (\$object) must be of type object, " . get_debug_type($object) . " given");
        }
    }
    // `spl_object_storage_attach`: an object already stored keeps its place.
    private function __attach($object, $info) {
        $k = $this->__key($object);
        if (array_key_exists($k, $this->storage)) {
            $this->storage[$k][1] = $info;
        } else {
            $this->storage[$k] = [$object, $info];
        }
    }
    // `spl_object_storage_detach`, then the pointer reset its callers make.
    private function __detach($object) {
        unset($this->storage[$this->__key($object)]);
        $this->pos = 0;
        $this->index = 0;
    }
    private function __at() {
        $keys = array_keys($this->storage);
        return $this->pos < count($keys) ? $this->storage[$keys[$this->pos]] : null;
    }
    private function __members() {
        $m = (array) $this;
        unset($m["\0SplObjectStorage\0storage"], $m["\0SplObjectStorage\0pos"], $m["\0SplObjectStorage\0index"]);
        return $m;
    }
    // The reference marks attach(), detach() and contains() `#[\Deprecated]`
    // (since 8.5). Withheld here: tests/spl.rs pins their output without the
    // deprecation — see BUGS.md.
    public function attach(object $object, mixed $info = null): void { $this->__attach($object, $info); }
    public function detach(object $object): void { $this->__detach($object); }
    public function contains(object $object): bool { return array_key_exists($this->__key($object), $this->storage); }
    public function addAll(SplObjectStorage $storage): int {
        foreach ($storage->__pairs() as [$object, $info]) { $this->__attach($object, $info); }
        $this->index = 0;
        return count($this->storage);
    }
    public function removeAll(SplObjectStorage $storage): int {
        foreach ($storage->__pairs() as [$object]) { unset($this->storage[$this->__key($object)]); }
        $this->pos = 0;
        $this->index = 0;
        return count($this->storage);
    }
    public function removeAllExcept(SplObjectStorage $storage): int {
        foreach ($this->storage as $k => [$object]) {
            if (!$storage->offsetExists($object)) { unset($this->storage[$this->__key($object)]); }
        }
        $this->pos = 0;
        $this->index = 0;
        return count($this->storage);
    }
    private function __pairs() { return array_values($this->storage); }
    public function getInfo(): mixed { return $this->__at()[1] ?? null; }
    public function setInfo(mixed $info): void {
        $keys = array_keys($this->storage);
        if ($this->pos < count($keys)) { $this->storage[$keys[$this->pos]][1] = $info; }
    }
    public function count(int $mode = COUNT_NORMAL): int { return count($this->storage); }
    public function rewind(): void { $this->pos = 0; $this->index = 0; }
    public function valid(): bool { return $this->pos < count($this->storage); }
    public function key(): int { return $this->index; }
    public function current(): object {
        $e = $this->__at();
        if ($e === null) { throw new RuntimeException("Called current() on invalid iterator"); }
        return $e[0];
    }
    public function next(): void {
        if ($this->pos < count($this->storage)) { $this->pos++; }
        $this->index++;
    }
    public function seek(int $offset): void {
        $n = count($this->storage);
        if ($offset < 0 || $offset >= $n) { throw new OutOfBoundsException("Seek position $offset is out of range"); }
        $forward = function () use ($n) { if ($this->pos < $n) { $this->pos++; } $this->index++; };
        if ($offset == 0) {
            $this->pos = 0;
            $this->index = 0;
        } elseif ($offset > $this->index) {
            do { $forward(); } while ($offset > $this->index);
        } elseif ($offset < $this->index) {
            if ($this->index - $offset > $offset) {
                $this->pos = 0;
                $this->index = 0;
                do { $forward(); } while ($offset > $this->index);
            } else {
                do {
                    if ($this->pos < $n) { $this->pos = $this->pos == 0 ? $n : $this->pos - 1; }
                    $this->index--;
                } while ($offset < $this->index);
            }
        }
    }
    public function serialize(): string {
        $s = "x:i:" . count($this->storage) . ";";
        foreach ($this->storage as [$object, $info]) { $s .= serialize($object) . "," . serialize($info) . ";"; }
        return $s . "m:" . serialize($this->__members());
    }
    public function unserialize(string $data): void {}
    public function offsetExists($object): bool { $this->__object($object, 'offsetExists'); return array_key_exists($this->__key($object), $this->storage); }
    public function offsetGet($object): mixed {
        $this->__object($object, 'offsetGet');
        $k = $this->__key($object);
        if (!array_key_exists($k, $this->storage)) { throw new UnexpectedValueException("Object not found"); }
        return $this->storage[$k][1];
    }
    public function offsetSet($object, mixed $info = null): void { $this->__object($object, 'offsetSet'); $this->__attach($object, $info); }
    public function offsetUnset($object): void { $this->__object($object, 'offsetUnset'); $this->__detach($object); }
    public function getHash(object $object): string { return spl_object_hash($object); }
    public function __serialize(): array {
        $flat = [];
        foreach ($this->storage as [$object, $info]) { $flat[] = $object; $flat[] = $info; }
        return [$flat, $this->__members()];
    }
    public function __unserialize(array $data): void {
        if (!isset($data[0], $data[1]) || !is_array($data[0]) || !is_array($data[1])) {
            throw new UnexpectedValueException("Incomplete or ill-typed serialization data");
        }
        if (count($data[0]) % 2 != 0) { throw new UnexpectedValueException("Odd number of elements"); }
        $key = null;
        foreach ($data[0] as $v) {
            if ($key === null) { $key = [$v]; continue; }
            if (!is_object($key[0])) { throw new UnexpectedValueException("Non-object key"); }
            $this->__attach($key[0], $v);
            $key = null;
        }
        foreach ($data[1] as $k => $v) { $this->$k = $v; }
    }
    public function __debugInfo(): array {
        $m = $this->__members();
        $storage = [];
        foreach ($this->storage as [$object, $info]) { $storage[] = ["obj" => $object, "inf" => $info]; }
        $m["\0SplObjectStorage\0storage"] = $storage;
        return $m;
    }
}
// SplHeap, SplMinHeap, SplMaxHeap and SplPriorityQueue: a port of
// ext/spl/spl_heap.c. `$heap` is the binary heap array the reference sifts,
// with the reference's sift-up and delete-top walks, so elements that compare
// equal come out in the reference's order. The two class families share the
// heap mechanics through this trait; each says how two stored elements
// compare (`__elemCmp`). A `compare()` that throws leaves the operation to
// finish with every later comparison reading 0, then marks the heap corrupted
// and rethrows, as `spl_ptr_heap_insert`/`_delete_top` do.
// Each class declares the `$flags`, `$isCorrupted` and `$heap` the methods use.
trait __SplHeapOps {
    // id => [write-locked, exception a compare() threw during this operation]
    private static $__op = [];
    private function __state() { return self::$__op[spl_object_id($this)] ?? [false, null]; }
    private function __cmp($a, $b) {
        [$locked, $thrown] = $this->__state();
        if ($thrown !== null) { return 0; }
        [$ok, $r] = __phplang_try_call(fn() => $this->__elemCmp($a, $b));
        if ($ok) { return ((int) $r) <=> 0; }
        self::$__op[spl_object_id($this)] = [$locked, $r];
        return 0;
    }
    private function __begin() { self::$__op[spl_object_id($this)] = [true, null]; }
    private function __end() {
        $thrown = $this->__state()[1];
        unset(self::$__op[spl_object_id($this)]);
        if ($thrown !== null) {
            $this->isCorrupted = true;
            throw $thrown;
        }
    }
    // `spl_heap_consistency_validations`.
    private function __validate($write) {
        if ($this->isCorrupted) {
            throw new RuntimeException("Heap is corrupted, heap properties are no longer ensured.");
        }
        if ($write && $this->__state()[0]) {
            throw new RuntimeException("Heap cannot be changed when it is already being modified.");
        }
    }
    // `spl_ptr_heap_insert`: sift the new element up from the end.
    private function __insert($elem) {
        $this->__begin();
        $pos = count($this->heap);
        while ($pos > 0) {
            $parent = intdiv($pos - 1, 2);
            if ($this->__cmp($this->heap[$parent], $elem) >= 0) { break; }
            $this->heap[$pos] = $this->heap[$parent];
            $pos = $parent;
        }
        $this->heap[$pos] = $elem;
        $this->__end();
    }
    // `spl_ptr_heap_delete_top`: take the root, then sift the last element
    // down from it. Null for an empty heap.
    private function __deleteTop() {
        $n = count($this->heap);
        if ($n === 0) { return null; }
        $this->__begin();
        $top = $this->heap[0];
        $limit = intdiv($n - 1, 2);
        $count = $n - 1;
        $bottom = $this->heap[$count];
        for ($i = 0; $i < $limit; $i = $j) {
            $j = $i * 2 + 1;
            if ($j !== $count && $this->__cmp($this->heap[$j + 1], $this->heap[$j]) > 0) { $j++; }
            if ($this->__cmp($bottom, $this->heap[$j]) < 0) {
                $this->heap[$i] = $this->heap[$j];
            } else {
                break;
            }
        }
        array_pop($this->heap);
        if ($i !== $count) { $this->heap[$i] = $bottom; }
        $this->__end();
        return [$top];
    }
    public function count(): int { return count($this->heap); }
    public function isEmpty(): bool { return count($this->heap) === 0; }
    public function rewind(): void {}
    public function key(): int { return count($this->heap) - 1; }
    public function next(): void { $this->__validate(true); $this->__deleteTop(); }
    public function valid(): bool { return count($this->heap) !== 0; }
    public function recoverFromCorruption(): true { $this->isCorrupted = false; return true; }
    public function isCorrupted(): bool { return $this->isCorrupted; }
    private function __members() {
        $m = (array) $this;
        $self = self::class;
        unset($m["\0$self\0flags"], $m["\0$self\0isCorrupted"], $m["\0$self\0heap"]);
        return $m;
    }
    public function __debugInfo(): array {
        $m = $this->__members();
        $self = self::class;
        $m["\0$self\0flags"] = $this->flags;
        $m["\0$self\0isCorrupted"] = $this->isCorrupted;
        $m["\0$self\0heap"] = $this->heap;
        return $m;
    }
    public function __serialize(): array {
        $this->__validate(false);
        if ($this->__state()[0]) {
            throw new RuntimeException("Cannot serialize heap while it is being modified.");
        }
        return [$this->__members(), ["flags" => $this->flags, "heap_elements" => $this->heap]];
    }
    // `spl_heap_unserialize_internal_state`'s checks; `$elem` validates and
    // shapes one stored element, or answers null to refuse it.
    private function __restore(array $data, $flagsOk, $elem) {
        $bad = "Invalid serialization data for " . get_class($this) . " object";
        if (count($data) !== 2 || !isset($data[0]) || !is_array($data[0])) { throw new Exception($bad); }
        foreach ($data[0] as $k => $v) { $this->$k = $v; }
        $state = $data[1] ?? null;
        if (!is_array($state) || !is_int($state["flags"] ?? null) || !$flagsOk($state["flags"])
            || !is_array($state["heap_elements"] ?? null)) {
            throw new Exception($bad);
        }
        $this->flags = $state["flags"];
        foreach ($state["heap_elements"] as $v) {
            $e = $elem($v);
            if ($e === null) { throw new Exception($bad); }
            $this->__insert($e[0]);
        }
    }
}
abstract class SplHeap implements Iterator, Countable {
    use __SplHeapOps;
    private $flags = 0;
    private $isCorrupted = false;
    private $heap = [];
    abstract protected function compare(mixed $value1, mixed $value2): int;
    private function __elemCmp($a, $b) { return $this->compare($a, $b); }
    public function insert(mixed $value): true { $this->__validate(true); $this->__insert($value); return true; }
    public function extract(): mixed {
        $this->__validate(true);
        $top = $this->__deleteTop();
        if ($top === null) { throw new RuntimeException("Can't extract from an empty heap"); }
        return $top[0];
    }
    public function top(): mixed {
        $this->__validate(false);
        if (count($this->heap) === 0) { throw new RuntimeException("Can't peek at an empty heap"); }
        return $this->heap[0];
    }
    public function current(): mixed { return count($this->heap) === 0 ? null : $this->heap[0]; }
    public function __unserialize(array $data): void {
        $this->__validate(true);
        $this->__restore($data, fn($f) => $f === 0, fn($v) => [$v]);
        $this->__validate(false);
    }
}
class SplMinHeap extends SplHeap {
    protected function compare(mixed $value1, mixed $value2): int { return $value2 <=> $value1; }
}
class SplMaxHeap extends SplHeap {
    protected function compare(mixed $value1, mixed $value2): int { return $value1 <=> $value2; }
}
class SplPriorityQueue implements Iterator, Countable {
    use __SplHeapOps;
    private $flags = 1;
    private $isCorrupted = false;
    private $heap = [];
    const EXTR_BOTH = 3;
    const EXTR_PRIORITY = 2;
    const EXTR_DATA = 1;
    public function compare(mixed $priority1, mixed $priority2): int { return $priority1 <=> $priority2; }
    private function __elemCmp($a, $b) { return $this->compare($a["priority"], $b["priority"]); }
    // `spl_pqueue_extract_helper`.
    private function __shape($elem) {
        if (($this->flags & 3) === 3) { return $elem; }
        return ($this->flags & 1) ? $elem["data"] : $elem["priority"];
    }
    public function insert(mixed $value, mixed $priority): true {
        $this->__validate(true);
        $this->__insert(["data" => $value, "priority" => $priority]);
        return true;
    }
    public function setExtractFlags(int $flags): int {
        $flags &= 3;
        if (!$flags) { throw new RuntimeException("Must specify at least one extract flag"); }
        $this->flags = $flags;
        return $flags;
    }
    public function getExtractFlags(): int { return $this->flags; }
    public function top(): mixed {
        $this->__validate(false);
        if (count($this->heap) === 0) { throw new RuntimeException("Can't peek at an empty heap"); }
        return $this->__shape($this->heap[0]);
    }
    public function extract(): mixed {
        $this->__validate(true);
        $top = $this->__deleteTop();
        if ($top === null) { throw new RuntimeException("Can't extract from an empty heap"); }
        return $this->__shape($top[0]);
    }
    public function current(): mixed { return count($this->heap) === 0 ? null : $this->__shape($this->heap[0]); }
    public function __unserialize(array $data): void {
        $this->__restore(
            $data,
            fn($f) => ($f & 3) !== 0,
            fn($v) => is_array($v) && count($v) === 2 && array_key_exists("data", $v) && array_key_exists("priority", $v)
                ? [["data" => $v["data"], "priority" => $v["priority"]]] : null
        );
        $this->flags &= 3;
        $this->__validate(false);
    }
}
// The iterators of ext/spl/spl_iterators.c. Each instance's internal state —
// the reference's `spl_dual_it_object` / `spl_recursive_it_object` — lives in
// `__SplIt`, keyed by object id, so an instance shows no properties, as the
// reference's do not. `__SplIt`'s methods are the C file's static functions;
// the classes below are its PHP_METHODs. An inner iterator is driven through
// its Iterator methods, which is what the reference's `zend_user_iterator`
// does, including keeping the value `current()` answered until the iterator
// moves (`zend_user_it_get_current_data`).
// The `Serializable` form `spl_array_serialize` writes and
// `spl_array_unserialize` reads: `x:i:FLAGS;STORAGE;m:MEMBERS`, the storage
// and the members each in `serialize` notation.
final class __SplArraySerial {
    public static function format(int $flags, $storage, array $members): string {
        return "x:i:" . $flags . ";" . serialize($storage) . ";m:" . serialize($members);
    }
    // `[flags, storage, members]`, or null for an empty string (a no-op); a
    // malformed one is `UnexpectedValueException: Error at offset N of LEN
    // bytes`, N where the part that failed begins.
    public static function parse(string $data): ?array {
        if ($data === "") { return null; }
        $len = strlen($data);
        if (!preg_match('/\Ax:i:(\d+);/', $data, $m)) {
            throw new UnexpectedValueException("Error at offset 0 of $len bytes");
        }
        $start = strlen($m[0]);
        // The storage ends at the `;m:` whose prefix is one whole value.
        for ($at = strpos($data, ";m:", $start); $at !== false; $at = strpos($data, ";m:", $at + 1)) {
            $part = substr($data, $start, $at - $start);
            $storage = @unserialize($part);
            if (($storage !== false || $part === "b:0;") && serialize($storage) === $part) {
                $members = @unserialize(substr($data, $at + 3));
                if (!is_array($members)) {
                    throw new UnexpectedValueException("Error at offset $len of $len bytes");
                }
                return [(int) $m[1], $storage, $members];
            }
        }
        throw new UnexpectedValueException("Error at offset $start of $len bytes");
    }
}

final class __SplIt {
    private static $d = [];
    // RecursiveTreeIterator's prefix parts and postfix, set when the object is
    // created (`spl_RecursiveIteratorIterator_new_ex`), not by its constructor.
    private static $tree = [];
    private static $warning = null;

    // SPL_FETCH_AND_CHECK_DUAL_IT
    public static function id($o) {
        $id = spl_object_id($o);
        if (!isset(self::$d[$id])) {
            throw new Error("The object is in an invalid state as the parent constructor was not called");
        }
        return $id;
    }
    public static function get($o, $k) { return self::$d[self::id($o)][$k]; }
    public static function put($o, $k, $v) { self::$d[self::id($o)][$k] = $v; }

    // The `dit_type != DIT_Unknown` check at the top of `spl_dual_it_construct`.
    public static function once($o, $base) {
        if (isset(self::$d[spl_object_id($o)])) {
            throw new BadMethodCallException("$base::getIterator() must be called exactly once per instance");
        }
    }
    // The tail of `spl_dual_it_construct`: the inner object and the state.
    public static function construct($o, $inner, $extra = []) {
        self::$d[spl_object_id($o)] = $extra + [
            'inner' => $inner,
            'has' => false,
            'data' => null,
            'hasKey' => false,
            'key' => null,
            'pos' => 0,
        ];
    }
    // `spl_get_iterator_from_aggregate`.
    public static function fromAggregate($agg, $class) {
        $r = $agg->getIterator();
        if (!($r instanceof Traversable)) {
            throw new LogicException("$class::getIterator() must return an object that implements Traversable");
        }
        return $r;
    }
    // `zend_user_it_get_new_iterator`: an aggregate is iterated through the
    // iterator its getIterator() answers.
    public static function iteratorOf($t) {
        while (!($t instanceof Iterator)) {
            $t = self::fromAggregate($t, get_class($t));
        }
        return $t;
    }
    // IteratorIterator's arm of `spl_dual_it_construct`.
    public static function constructIteratorIterator($o, $it, $class) {
        self::once($o, 'IteratorIterator');
        if (!($it instanceof Iterator)) {
            $ce = get_class($it);
            if ($class !== null) {
                if (!class_exists($class) && !interface_exists($class) || !is_a($it, $class) || !is_subclass_of($class, 'Traversable') && strcasecmp($class, 'Traversable') !== 0) {
                    throw new LogicException("Class to downcast to not found or not base class or does not implement Traversable");
                }
                $ce = $class;
            }
            if ($it instanceof IteratorAggregate) {
                $it = self::fromAggregate($it, $ce);
            }
        }
        self::construct($o, $it, ['it' => self::iteratorOf($it)]);
    }

    // `spl_dual_it_free`
    public static function free($id) {
        self::$d[$id]['has'] = false;
        self::$d[$id]['data'] = null;
        self::$d[$id]['hasKey'] = false;
        self::$d[$id]['key'] = null;
        if (array_key_exists('zstr', self::$d[$id])) {
            self::$d[$id]['zstr'] = null;
            self::$d[$id]['children'] = null;
        }
    }
    // `spl_dual_it_rewind`
    public static function rewind($id) {
        self::free($id);
        self::$d[$id]['pos'] = 0;
        $it = self::$d[$id]['it'];
        if ($it !== null) { $it->rewind(); }
    }
    // `spl_dual_it_valid`
    public static function valid($id) {
        $it = self::$d[$id]['it'];
        return $it !== null && (bool) $it->valid();
    }
    // `spl_dual_it_fetch`
    public static function fetch($id, $checkMore) {
        self::free($id);
        if ($checkMore && !self::valid($id)) { return false; }
        $it = self::$d[$id]['it'];
        self::$d[$id]['data'] = $it->current();
        self::$d[$id]['has'] = true;
        self::$d[$id]['key'] = $it->key();
        self::$d[$id]['hasKey'] = true;
        return true;
    }
    // `spl_dual_it_next`
    public static function next($id, $doFree) {
        if ($doFree) {
            self::free($id);
        } elseif (self::$d[$id]['it'] === null) {
            throw new Error("The inner constructor wasn't initialized with an iterator instance");
        }
        self::$d[$id]['it']->next();
        self::$d[$id]['pos']++;
    }
    // `spl_filter_it_fetch`
    public static function filterFetch($o, $id) {
        while (self::fetch($id, true)) {
            if ($o->accept()) { return; }
            self::$d[$id]['it']->next();
        }
        self::free($id);
    }
    // `spl_dual_it_get_method`: a method the iterator does not have is looked
    // up on the inner iterator.
    public static function forward($o, $name, $args) {
        $inner = self::$d[spl_object_id($o)]['inner'] ?? null;
        if (is_object($inner) && (method_exists($inner, $name) || method_exists($inner, '__call'))) {
            return $inner->$name(...$args);
        }
        throw new Error("Call to undefined method " . get_class($o) . "::$name()");
    }

    // `spl_limit_it_seek`
    public static function limitSeek($o, $pos) {
        $id = self::id($o);
        $s = self::$d[$id];
        self::free($id);
        if ($pos < $s['offset']) {
            throw new OutOfBoundsException("Cannot seek to $pos which is below the offset {$s['offset']}");
        }
        if ($pos - $s['offset'] >= $s['count'] && $s['count'] != -1) {
            throw new OutOfBoundsException("Cannot seek to $pos which is behind offset {$s['offset']} plus count {$s['count']}");
        }
        if ($pos != $s['pos'] && $s['it'] instanceof SeekableIterator) {
            self::free($id);
            $s['it']->seek($pos);
            self::$d[$id]['pos'] = $pos;
            if (self::limitValid($id)) { self::fetch($id, false); }
        } else {
            if ($pos < $s['pos']) { self::rewind($id); }
            while ($pos > self::$d[$id]['pos'] && self::valid($id)) { self::next($id, true); }
            if (self::valid($id)) { self::fetch($id, true); }
        }
    }
    // `spl_limit_it_valid`
    public static function limitValid($id) {
        $s = self::$d[$id];
        if ($s['count'] != -1 && $s['pos'] - $s['offset'] >= $s['count']) { return false; }
        return self::valid($id);
    }

    // `spl_cit_check_flags`
    public static function citFlagsOk($flags) {
        $n = 0;
        foreach ([1, 2, 4, 8] as $bit) { if ($flags & $bit) { $n++; } }
        return $n <= 1;
    }
    public static function citFlagsError($fn, $arg) {
        throw new ValueError("$fn(): Argument #$arg (\$flags) must contain only one of CachingIterator::CALL_TOSTRING, CachingIterator::TOSTRING_USE_KEY, CachingIterator::TOSTRING_USE_CURRENT, or CachingIterator::TOSTRING_USE_INNER");
    }
    // `spl_caching_it_next`
    public static function cachingNext($o, $id) {
        if (!self::fetch($id, true)) {
            self::$d[$id]['flags'] &= ~0x10000;
            return;
        }
        self::$d[$id]['flags'] |= 0x10000;
        $flags = self::$d[$id]['flags'];
        if ($flags & 0x100) {
            $cache = self::$d[$id]['cache'];
            $cache[self::$d[$id]['key']] = self::$d[$id]['data'];
            self::$d[$id]['cache'] = $cache;
        }
        if (self::$d[$id]['recursive']) {
            $inner = self::$d[$id]['inner'];
            [$ok, $has] = __phplang_try_call(fn() => $inner->hasChildren());
            if (!$ok) {
                if (!($flags & 0x10)) { throw $has; }
            } elseif ($has) {
                [$ok, $kids] = __phplang_try_call(fn() => $inner->getChildren());
                if (!$ok) {
                    if (!($flags & 0x10)) { throw $kids; }
                } else {
                    [$ok, $child] = __phplang_try_call(fn() => new RecursiveCachingIterator($kids, $flags & 0xFFFF));
                    if ($ok) {
                        self::$d[$id]['children'] = $child;
                    } elseif (!($flags & 0x10)) {
                        throw $child;
                    }
                }
            }
        }
        if ($flags & (8 | 1)) {
            self::$d[$id]['zstr'] = ($flags & 8) ? (string) self::$d[$id]['inner'] : (string) self::$d[$id]['data'];
        }
        self::next($id, false);
    }
    // The `CIT_FULL_CACHE` check every cache accessor starts with.
    public static function fullCache($o) {
        $id = self::id($o);
        if (!(self::$d[$id]['flags'] & 0x100)) {
            throw new BadMethodCallException(get_class($o) . " does not use a full cache (see CachingIterator::__construct)");
        }
        return $id;
    }
    public static function cacheGet($o, $key) {
        $id = self::fullCache($o);
        $key = (string) $key;
        if (!array_key_exists($key, self::$d[$id]['cache'])) {
            __phplang_warn("Undefined array key \"$key\"");
            return null;
        }
        return self::$d[$id]['cache'][$key];
    }
    public static function cacheSet($o, $key, $value) {
        $id = self::fullCache($o);
        self::$d[$id]['cache'][(string) $key] = $value;
    }
    public static function cacheUnset($o, $key) {
        $id = self::fullCache($o);
        unset(self::$d[$id]['cache'][(string) $key]);
    }

    // `spl_append_it_next_iterator`
    public static function appendNextIterator($id) {
        self::free($id);
        self::$d[$id]['inner'] = null;
        self::$d[$id]['it'] = null;
        $list = self::$d[$id]['list'];
        if (!$list->valid()) { return false; }
        $it = $list->current();
        self::$d[$id]['inner'] = $it;
        self::$d[$id]['it'] = $it;
        self::rewind($id);
        return true;
    }
    // `spl_append_it_fetch`
    public static function appendFetch($id) {
        while (!self::valid($id)) {
            self::$d[$id]['list']->next();
            if (!self::appendNextIterator($id)) { return; }
        }
        self::fetch($id, false);
    }
    // AppendIterator::append
    public static function append($o, $it) {
        $id = self::id($o);
        $list = self::$d[$id]['list'];
        if ($list->valid() && !self::valid($id)) {
            $list->append($it);
            $list->next();
        } else {
            $list->append($it);
        }
        if (self::$d[$id]['it'] === null || !self::valid($id)) {
            if (!$list->valid()) { $list->rewind(); }
            do {
                self::appendNextIterator($id);
            } while (self::$d[$id]['inner'] !== $it);
            self::appendFetch($id);
        }
    }

    // A REGIT_MODE_* argument outside the five modes.
    public static function regexModeError($fn, $arg) {
        throw new ValueError("$fn(): Argument #$arg (\$mode) must be RegexIterator::MATCH, RegexIterator::GET_MATCH, RegexIterator::ALL_MATCHES, RegexIterator::SPLIT, or RegexIterator::REPLACE");
    }
    public static function catchWarning($no, $msg, $file = null, $line = null) {
        self::$warning = $msg;
        return true;
    }
    // `pcre_get_compiled_regex_cache` under EH_THROW: a pattern that does not
    // compile is an InvalidArgumentException carrying the warning, reworded
    // as the constructor's.
    public static function compileRegex($regex, $fn) {
        self::$warning = null;
        set_error_handler([self::class, 'catchWarning']);
        preg_match($regex, '');
        restore_error_handler();
        if (self::$warning !== null) {
            $msg = self::$warning;
            self::$warning = null;
            throw new InvalidArgumentException("$fn(): " . substr($msg, strlen('preg_match(): ')));
        }
    }
    // RegexIterator::accept
    public static function regexAccept($o) {
        $id = self::id($o);
        $s = self::$d[$id];
        if (!$s['has']) { return false; }
        if ($s['flags'] & 1) {
            $subject = (string) $s['key'];
        } else {
            if (is_array($s['data'])) { return false; }
            $subject = (string) $s['data'];
        }
        switch ($s['mode']) {
            case 1:
            case 2:
                self::$d[$id]['data'] = null;
                $count = $s['mode'] === 2
                    ? preg_match_all($s['regex'], $subject, $m, $s['pflags'])
                    : preg_match($s['regex'], $subject, $m, $s['pflags']);
                self::$d[$id]['data'] = $m;
                $r = $count > 0;
                break;
            case 3:
                self::$d[$id]['data'] = null;
                $parts = preg_split($s['regex'], $subject, -1, $s['pflags']);
                self::$d[$id]['data'] = $parts;
                $r = count($parts) > 1;
                break;
            case 4:
                $result = preg_replace($s['regex'], (string) $o->replacement, $subject, -1, $count);
                if ($result === null) { return false; }
                if ($s['flags'] & 1) {
                    self::$d[$id]['key'] = $result;
                } else {
                    self::$d[$id]['data'] = $result;
                }
                $r = $count > 0;
                break;
            default:
                $r = preg_match($s['regex'], $subject) === 1;
        }
        if ($s['flags'] & 2) { return !$r; }
        return $r;
    }

    // --- RecursiveIteratorIterator: `spl_recursive_it_object` ---
    // Each level is [iterator, state, value cached?, cached value].
    // States: RS_NEXT 0, RS_TEST 1, RS_SELF 2, RS_CHILD 3, RS_START 4.

    // `spl_recursive_it_it_construct`
    public static function riConstruct($o, $iterator, $mode, $flags) {
        if ($iterator instanceof IteratorAggregate) {
            $iterator = self::fromAggregate($iterator, get_class($iterator));
        }
        if (!($iterator instanceof RecursiveIterator)) {
            throw new InvalidArgumentException("An instance of RecursiveIterator or IteratorAggregate creating it is required");
        }
        // The hooks a subclass overrides; the base class's are not called.
        $hooks = [];
        foreach (['beginiteration', 'enditeration', 'callhaschildren', 'callgetchildren', 'beginchildren', 'endchildren', 'nextelement'] as $h) {
            $owner = __phplang_method_owner($o, $h);
            $hooks[$h] = $owner !== 'recursiveiteratoriterator' && $owner !== 'recursivetreeiterator';
        }
        self::$d[spl_object_id($o)] = [
            'its' => [[$iterator, 4, false, null]],
            'level' => 0,
            'mode' => $mode,
            'flags' => $flags,
            'max' => -1,
            'inIter' => false,
            'hooks' => $hooks,
        ];
    }
    // SPL_FETCH_SUB_ITERATOR
    public static function riId($o) { return self::id($o); }
    public static function riLevelIt($o) {
        $id = self::id($o);
        return self::$d[$id]['its'][self::$d[$id]['level']][0];
    }
    public static function riDepth($o) { return self::$d[spl_object_id($o)]['level'] ?? 0; }
    public static function riSubIterator($o, $level) {
        $id = spl_object_id($o);
        $cur = self::$d[$id]['level'] ?? 0;
        if ($level === null) {
            $level = $cur;
        } elseif ($level < 0 || $level > $cur) {
            return null;
        }
        self::id($o);
        return self::$d[$id]['its'][$level][0];
    }
    public static function riHasChildren($o) {
        $id = spl_object_id($o);
        if (!isset(self::$d[$id])) { return false; }
        return self::$d[$id]['its'][self::$d[$id]['level']][0]->hasChildren();
    }
    public static function riSetMax($o, $max) {
        if ($max < -1) {
            throw new ValueError("RecursiveIteratorIterator::setMaxDepth(): Argument #1 (\$maxDepth) must be greater than or equal to -1");
        }
        if ($max > 2147483647) { $max = 2147483647; }
        self::$d[self::id($o)]['max'] = $max;
    }
    public static function riGetMax($o) {
        $max = self::$d[spl_object_id($o)]['max'] ?? 0;
        return $max === -1 ? false : $max;
    }
    // `get_current_data` of the active sub-iterator, which keeps the value
    // until that iterator moves.
    public static function riCurrent($o) {
        $id = self::id($o);
        $l = self::$d[$id]['level'];
        if (!self::$d[$id]['its'][$l][2]) {
            self::$d[$id]['its'][$l][3] = self::$d[$id]['its'][$l][0]->current();
            self::$d[$id]['its'][$l][2] = true;
        }
        return self::$d[$id]['its'][$l][3];
    }
    // `spl_recursive_it_valid_ex`
    public static function riValid($o) {
        $id = spl_object_id($o);
        if (!isset(self::$d[$id])) { return false; }
        for ($l = self::$d[$id]['level']; $l >= 0; $l--) {
            if (self::$d[$id]['its'][$l][0]->valid()) { return true; }
        }
        if (self::$d[$id]['hooks']['enditeration'] && self::$d[$id]['inIter']) {
            [$ok, $e] = __phplang_try_call(fn() => $o->endIteration());
            self::$d[$id]['inIter'] = false;
            if (!$ok) { throw $e; }
        }
        self::$d[$id]['inIter'] = false;
        return false;
    }
    // `spl_recursive_it_rewind_ex`
    public static function riRewind($o) {
        $id = self::id($o);
        $thrown = null;
        while (self::$d[$id]['level'] > 0) {
            $l = self::$d[$id]['level'];
            unset(self::$d[$id]['its'][$l]);
            self::$d[$id]['level'] = $l - 1;
            if ($thrown === null) {
                [$ok, $e] = __phplang_try_call(fn() => $o->endChildren());
                if (!$ok) { $thrown = $e; }
            }
        }
        self::$d[$id]['its'][0][1] = 4;
        self::$d[$id]['its'][0][2] = false;
        $it = self::$d[$id]['its'][0][0];
        if ($thrown === null) {
            [$ok, $e] = __phplang_try_call(fn() => $it->rewind());
            if (!$ok) { $thrown = $e; }
        }
        if ($thrown === null && self::$d[$id]['hooks']['beginiteration'] && !self::$d[$id]['inIter']) {
            [$ok, $e] = __phplang_try_call(fn() => $o->beginIteration());
            if (!$ok) { $thrown = $e; }
        }
        self::$d[$id]['inIter'] = true;
        if ($thrown !== null) { throw $thrown; }
        self::riForward($o);
    }
    // `spl_recursive_it_move_forward_ex`
    public static function riForward($o) {
        $id = self::id($o);
        $catch = (self::$d[$id]['flags'] & 16) !== 0;
        $mode = self::$d[$id]['mode'];
        $hooks = self::$d[$id]['hooks'];
        while (true) {
            $l = self::$d[$id]['level'];
            $it = self::$d[$id]['its'][$l][0];
            $state = self::$d[$id]['its'][$l][1];
            if ($state === 0) {
                self::$d[$id]['its'][$l][2] = false;
                [$ok, $e] = __phplang_try_call(fn() => $it->next());
                if (!$ok && !$catch) { throw $e; }
                $state = 4;
            }
            if ($state === 4) {
                $valid = $it->valid();
                if (self::$d[$id]['level'] !== $l || (self::$d[$id]['its'][$l][0] ?? null) !== $it) {
                    return;
                }
                if ($valid) {
                    self::$d[$id]['its'][$l][1] = 1;
                    $state = 1;
                }
            }
            if ($state === 1) {
                [$ok, $has] = $hooks['callhaschildren']
                    ? __phplang_try_call(fn() => $o->callHasChildren())
                    : __phplang_try_call(fn() => $it->hasChildren());
                if (!$ok) {
                    if (!$catch) {
                        self::$d[$id]['its'][$l][1] = 0;
                        throw $has;
                    }
                    $has = false;
                }
                if ($has) {
                    $max = self::$d[$id]['max'];
                    if ($max === -1 || $max > $l) {
                        if ($mode === 0 || $mode === 2) {
                            self::$d[$id]['its'][$l][1] = 3;
                            continue;
                        }
                        if ($mode === 1) {
                            self::$d[$id]['its'][$l][1] = 2;
                            continue;
                        }
                    } elseif ($mode === 0) {
                        self::$d[$id]['its'][$l][1] = 0;
                        continue;
                    }
                }
                $ok = true;
                if ($hooks['nextelement']) {
                    [$ok, $e] = __phplang_try_call(fn() => $o->nextElement());
                }
                self::$d[$id]['its'][$l][1] = 0;
                if (!$ok && !$catch) { throw $e; }
                return;
            }
            if ($state === 2) {
                $ok = true;
                if ($hooks['nextelement'] && ($mode === 1 || $mode === 2)) {
                    [$ok, $e] = __phplang_try_call(fn() => $o->nextElement());
                }
                self::$d[$id]['its'][$l][1] = $mode === 1 ? 3 : 0;
                if (!$ok) { throw $e; }
                return;
            }
            if ($state === 3) {
                [$ok, $child] = $hooks['callgetchildren']
                    ? __phplang_try_call(fn() => $o->callGetChildren())
                    : __phplang_try_call(fn() => $it->getChildren());
                if (!$ok) {
                    if (!$catch) { throw $child; }
                    self::$d[$id]['its'][$l][1] = 0;
                    continue;
                }
                if (!($child instanceof RecursiveIterator)) {
                    throw new UnexpectedValueException("Objects returned by RecursiveIterator::getChildren() must implement RecursiveIterator");
                }
                self::$d[$id]['its'][$l][1] = $mode === 2 ? 2 : 0;
                self::$d[$id]['level'] = $l + 1;
                self::$d[$id]['its'][$l + 1] = [$child, 4, false, null];
                $child->rewind();
                if ($hooks['beginchildren']) {
                    [$ok, $e] = __phplang_try_call(fn() => $o->beginChildren());
                    if (!$ok && !$catch) { throw $e; }
                }
                continue;
            }
            // no more elements at this level
            if ($l > 0) {
                if ($hooks['endchildren']) {
                    [$ok, $e] = __phplang_try_call(fn() => $o->endChildren());
                    if (!$ok && !$catch) { throw $e; }
                }
                $cur = self::$d[$id]['level'];
                if ($cur > 0 && self::$d[$id]['its'][$cur][0] === $it) {
                    unset(self::$d[$id]['its'][$cur]);
                    self::$d[$id]['level'] = $cur - 1;
                }
            } else {
                return;
            }
        }
    }
    // `spl_recursive_it_get_method`
    public static function riForward_method($o, $name, $args) {
        $id = spl_object_id($o);
        if (!isset(self::$d[$id])) {
            throw new Error("The " . get_class($o) . " instance wasn't initialized properly");
        }
        $sub = self::$d[$id]['its'][self::$d[$id]['level']][0];
        if (method_exists($sub, $name) || method_exists($sub, '__call')) {
            return $sub->$name(...$args);
        }
        throw new Error("Call to undefined method " . get_class($o) . "::$name()");
    }

    // --- RecursiveTreeIterator ---
    public static function treeParts($o) {
        return self::$tree[spl_object_id($o)] ?? [["", "| ", "  ", "|-", "\\-", ""], ""];
    }
    public static function treeSetPart($o, $part, $value) {
        if ($part < 0 || $part > 5) {
            throw new ValueError("RecursiveTreeIterator::setPrefixPart(): Argument #1 (\$part) must be a RecursiveTreeIterator::PREFIX_* constant");
        }
        $t = self::treeParts($o);
        $t[0][$part] = $value;
        self::$tree[spl_object_id($o)] = $t;
    }
    public static function treeSetPostfix($o, $postfix) {
        $t = self::treeParts($o);
        $t[1] = $postfix;
        self::$tree[spl_object_id($o)] = $t;
    }
    // `spl_recursive_tree_iterator_get_prefix`
    public static function treePrefix($o) {
        $id = self::id($o);
        [$prefix] = self::treeParts($o);
        $str = $prefix[0];
        $level = self::$d[$id]['level'];
        for ($l = 0; $l < $level; $l++) {
            $str .= self::$d[$id]['its'][$l][0]->hasNext() === true ? $prefix[1] : $prefix[2];
        }
        $str .= self::$d[$id]['its'][$level][0]->hasNext() === true ? $prefix[3] : $prefix[4];
        return $str . $prefix[5];
    }
    // `spl_recursive_tree_iterator_get_entry`
    public static function treeEntry($o) {
        $data = self::riCurrent($o);
        return is_array($data) ? "Array" : (string) $data;
    }
}
class EmptyIterator implements Iterator {
    public function current(): never { throw new BadMethodCallException("Accessing the value of an EmptyIterator"); }
    public function next(): void {}
    public function key(): never { throw new BadMethodCallException("Accessing the key of an EmptyIterator"); }
    public function valid(): false { return false; }
    public function rewind(): void {}
}
class IteratorIterator implements OuterIterator {
    public function __construct(Traversable $iterator, ?string $class = null) { __SplIt::constructIteratorIterator($this, $iterator, $class); }
    public function getInnerIterator(): ?Iterator { return __SplIt::get($this, 'inner'); }
    public function rewind(): void { $id = __SplIt::id($this); __SplIt::rewind($id); __SplIt::fetch($id, true); }
    public function valid(): bool { return __SplIt::get($this, 'has'); }
    public function key(): mixed { return __SplIt::get($this, 'key'); }
    public function current(): mixed { return __SplIt::get($this, 'data'); }
    public function next(): void { $id = __SplIt::id($this); __SplIt::next($id, true); __SplIt::fetch($id, true); }
    public function __call($name, $args) { return __SplIt::forward($this, $name, $args); }
}
abstract class FilterIterator extends IteratorIterator {
    abstract public function accept(): bool;
    public function __construct(Iterator $iterator) { __SplIt::once($this, 'FilterIterator'); __SplIt::construct($this, $iterator, ['it' => $iterator]); }
    public function rewind(): void { $id = __SplIt::id($this); __SplIt::rewind($id); __SplIt::filterFetch($this, $id); }
    public function next(): void { $id = __SplIt::id($this); __SplIt::next($id, true); __SplIt::filterFetch($this, $id); }
}
class CallbackFilterIterator extends FilterIterator {
    public function __construct(Iterator $iterator, callable $callback) { __SplIt::once($this, 'CallbackFilterIterator'); __SplIt::construct($this, $iterator, ['it' => $iterator, 'callback' => $callback]); }
    public function accept(): bool {
        $id = __SplIt::id($this);
        if (!__SplIt::get($this, 'has') || !__SplIt::get($this, 'hasKey')) { return false; }
        $callback = __SplIt::get($this, 'callback');
        return $callback(__SplIt::get($this, 'data'), __SplIt::get($this, 'key'), __SplIt::get($this, 'inner'));
    }
}
abstract class RecursiveFilterIterator extends FilterIterator implements RecursiveIterator {
    public function __construct(RecursiveIterator $iterator) { __SplIt::once($this, 'RecursiveFilterIterator'); __SplIt::construct($this, $iterator, ['it' => $iterator]); }
    public function hasChildren(): bool { return __SplIt::get($this, 'inner')->hasChildren(); }
    public function getChildren(): ?RecursiveFilterIterator { return new static(__SplIt::get($this, 'inner')->getChildren()); }
}
class RecursiveCallbackFilterIterator extends CallbackFilterIterator implements RecursiveIterator {
    public function __construct(RecursiveIterator $iterator, callable $callback) { __SplIt::once($this, 'RecursiveCallbackFilterIterator'); __SplIt::construct($this, $iterator, ['it' => $iterator, 'callback' => $callback]); }
    public function hasChildren(): bool { return __SplIt::get($this, 'inner')->hasChildren(); }
    public function getChildren(): RecursiveCallbackFilterIterator {
        $children = __SplIt::get($this, 'inner')->getChildren();
        return new static($children, __SplIt::get($this, 'callback'));
    }
}
class ParentIterator extends RecursiveFilterIterator {
    public function __construct(RecursiveIterator $iterator) { __SplIt::once($this, 'ParentIterator'); __SplIt::construct($this, $iterator, ['it' => $iterator]); }
    public function accept(): bool { return __SplIt::get($this, 'inner')->hasChildren(); }
}
class LimitIterator extends IteratorIterator {
    public function __construct(Iterator $iterator, int $offset = 0, int $limit = -1) {
        __SplIt::once($this, 'LimitIterator');
        if ($offset < 0) { throw new ValueError("LimitIterator::__construct(): Argument #2 (\$offset) must be greater than or equal to 0"); }
        if ($limit < -1) { throw new ValueError("LimitIterator::__construct(): Argument #3 (\$limit) must be greater than or equal to -1"); }
        __SplIt::construct($this, $iterator, ['it' => $iterator, 'offset' => $offset, 'count' => $limit]);
    }
    public function rewind(): void { $id = __SplIt::id($this); __SplIt::rewind($id); __SplIt::limitSeek($this, __SplIt::get($this, 'offset')); }
    public function valid(): bool {
        $id = __SplIt::id($this);
        $count = __SplIt::get($this, 'count');
        return ($count == -1 || __SplIt::get($this, 'pos') - __SplIt::get($this, 'offset') < $count) && __SplIt::get($this, 'has');
    }
    public function next(): void {
        $id = __SplIt::id($this);
        __SplIt::next($id, true);
        $count = __SplIt::get($this, 'count');
        if ($count == -1 || __SplIt::get($this, 'pos') - __SplIt::get($this, 'offset') < $count) { __SplIt::fetch($id, true); }
    }
    public function seek(int $offset): int { __SplIt::id($this); __SplIt::limitSeek($this, $offset); return __SplIt::get($this, 'pos'); }
    public function getPosition(): int { return __SplIt::get($this, 'pos'); }
}
class CachingIterator extends IteratorIterator implements ArrayAccess, Countable, Stringable {
    const CALL_TOSTRING = 1;
    const CATCH_GET_CHILD = 16;
    const TOSTRING_USE_KEY = 2;
    const TOSTRING_USE_CURRENT = 4;
    const TOSTRING_USE_INNER = 8;
    const FULL_CACHE = 256;
    public function __construct(Iterator $iterator, int $flags = CachingIterator::CALL_TOSTRING) {
        __SplIt::once($this, 'CachingIterator');
        if (!__SplIt::citFlagsOk($flags)) { __SplIt::citFlagsError('CachingIterator::__construct', 2); }
        __SplIt::construct($this, $iterator, ['it' => $iterator, 'flags' => $flags & 0xFFFF, 'cache' => [], 'zstr' => null, 'children' => null, 'recursive' => false]);
    }
    public function rewind(): void { $id = __SplIt::id($this); __SplIt::rewind($id); __SplIt::put($this, 'cache', []); __SplIt::cachingNext($this, $id); }
    public function valid(): bool { return (__SplIt::get($this, 'flags') & 0x10000) !== 0; }
    public function next(): void { __SplIt::cachingNext($this, __SplIt::id($this)); }
    public function hasNext(): bool { return __SplIt::valid(__SplIt::id($this)); }
    public function __toString(): string {
        $flags = __SplIt::get($this, 'flags');
        if (!($flags & (1 | 2 | 4 | 8))) {
            throw new BadMethodCallException(get_class($this) . " does not fetch string value (see CachingIterator::__construct)");
        }
        if ($flags & 2) { return (string) __SplIt::get($this, 'key'); }
        if ($flags & 4) { return (string) __SplIt::get($this, 'data'); }
        return (string) __SplIt::get($this, 'zstr');
    }
    public function getFlags(): int { return __SplIt::get($this, 'flags'); }
    public function setFlags(int $flags): void {
        $old = __SplIt::get($this, 'flags');
        if (!__SplIt::citFlagsOk($flags)) { __SplIt::citFlagsError('CachingIterator::setFlags', 1); }
        if (($old & 1) && !($flags & 1)) { throw new InvalidArgumentException("Unsetting flag CALL_TO_STRING is not possible"); }
        if (($old & 8) && !($flags & 8)) { throw new InvalidArgumentException("Unsetting flag TOSTRING_USE_INNER is not possible"); }
        if (($flags & 0x100) && !($old & 0x100)) { __SplIt::put($this, 'cache', []); }
        __SplIt::put($this, 'flags', ($old & ~0xFFFF) | ($flags & 0xFFFF));
    }
    public function offsetGet($key): mixed { return __SplIt::cacheGet($this, $key); }
    public function offsetSet($key, mixed $value): void { __SplIt::cacheSet($this, $key, $value); }
    public function offsetUnset($key): void { __SplIt::cacheUnset($this, $key); }
    public function offsetExists($key): bool { $id = __SplIt::fullCache($this); return array_key_exists((string) $key, __SplIt::get($this, 'cache')); }
    public function getCache(): array { __SplIt::fullCache($this); return __SplIt::get($this, 'cache'); }
    public function count(): int { __SplIt::fullCache($this); return count(__SplIt::get($this, 'cache')); }
}
class RecursiveCachingIterator extends CachingIterator implements RecursiveIterator {
    public function __construct($iterator, int $flags = RecursiveCachingIterator::CALL_TOSTRING) {
        __SplIt::once($this, 'RecursiveCachingIterator');
        if (!($iterator instanceof RecursiveIterator)) {
            throw new TypeError("RecursiveCachingIterator::__construct(): Argument #1 (\$iterator) must be of type RecursiveIterator, " . get_debug_type($iterator) . " given");
        }
        if (!__SplIt::citFlagsOk($flags)) { __SplIt::citFlagsError('RecursiveCachingIterator::__construct', 2); }
        __SplIt::construct($this, $iterator, ['it' => $iterator, 'flags' => $flags & 0xFFFF, 'cache' => [], 'zstr' => null, 'children' => null, 'recursive' => true]);
    }
    public function hasChildren(): bool { return __SplIt::get($this, 'children') !== null; }
    public function getChildren(): ?RecursiveCachingIterator { return __SplIt::get($this, 'children'); }
}
class NoRewindIterator extends IteratorIterator {
    public function __construct(Iterator $iterator) { __SplIt::once($this, 'NoRewindIterator'); __SplIt::construct($this, $iterator, ['it' => $iterator, 'cached' => false]); }
    public function rewind(): void {}
    public function valid(): bool { return (bool) __SplIt::get($this, 'inner')->valid(); }
    public function key(): mixed { return __SplIt::get($this, 'inner')->key(); }
    public function current(): mixed {
        if (!__SplIt::get($this, 'cached')) {
            __SplIt::put($this, 'data', __SplIt::get($this, 'inner')->current());
            __SplIt::put($this, 'cached', true);
        }
        return __SplIt::get($this, 'data');
    }
    public function next(): void { __SplIt::put($this, 'cached', false); __SplIt::get($this, 'inner')->next(); }
}
class InfiniteIterator extends IteratorIterator {
    public function __construct(Iterator $iterator) { __SplIt::once($this, 'InfiniteIterator'); __SplIt::construct($this, $iterator, ['it' => $iterator]); }
    public function next(): void {
        $id = __SplIt::id($this);
        __SplIt::next($id, true);
        if (__SplIt::valid($id)) {
            __SplIt::fetch($id, false);
        } else {
            __SplIt::rewind($id);
            if (__SplIt::valid($id)) { __SplIt::fetch($id, false); }
        }
    }
}
class AppendIterator extends IteratorIterator {
    public function __construct() { __SplIt::once($this, 'AppendIterator'); __SplIt::construct($this, null, ['it' => null, 'list' => new ArrayIterator()]); }
    public function append(Iterator $iterator): void { __SplIt::append($this, $iterator); }
    public function rewind(): void {
        $id = __SplIt::id($this);
        __SplIt::get($this, 'list')->rewind();
        if (__SplIt::appendNextIterator($id)) { __SplIt::appendFetch($id); }
    }
    public function valid(): bool { return __SplIt::get($this, 'has'); }
    public function current(): mixed { $id = __SplIt::id($this); __SplIt::fetch($id, true); return __SplIt::get($this, 'data'); }
    public function next(): void {
        $id = __SplIt::id($this);
        if (__SplIt::valid($id)) { __SplIt::next($id, true); }
        __SplIt::appendFetch($id);
    }
    public function getIteratorIndex(): ?int {
        return __SplIt::get($this, 'list')->key();
    }
    public function getArrayIterator(): ArrayIterator { return __SplIt::get($this, 'list'); }
}
class RegexIterator extends FilterIterator {
    const USE_KEY = 1;
    const INVERT_MATCH = 2;
    const MATCH = 0;
    const GET_MATCH = 1;
    const ALL_MATCHES = 2;
    const SPLIT = 3;
    const REPLACE = 4;
    public ?string $replacement = null;
    public function __construct(Iterator $iterator, string $pattern, int $mode = RegexIterator::MATCH, int $flags = 0, int $pregFlags = 0) {
        __SplIt::once($this, 'RegexIterator');
        if ($mode < 0 || $mode >= 5) { __SplIt::regexModeError('RegexIterator::__construct', 3); }
        __SplIt::compileRegex($pattern, 'RegexIterator::__construct');
        __SplIt::construct($this, $iterator, ['it' => $iterator, 'regex' => $pattern, 'mode' => $mode, 'flags' => $flags, 'pflags' => $pregFlags]);
    }
    public function accept(): bool { return __SplIt::regexAccept($this); }
    public function getMode(): int { return __SplIt::get($this, 'mode'); }
    public function setMode(int $mode): void {
        if ($mode < 0 || $mode >= 5) { __SplIt::regexModeError('RegexIterator::setMode', 1); }
        __SplIt::put($this, 'mode', $mode);
    }
    public function getFlags(): int { return __SplIt::get($this, 'flags'); }
    public function setFlags(int $flags): void { __SplIt::put($this, 'flags', $flags); }
    public function getRegex(): string { return __SplIt::get($this, 'regex'); }
    public function getPregFlags(): int { return __SplIt::get($this, 'pflags'); }
    public function setPregFlags(int $pregFlags): void { __SplIt::put($this, 'pflags', $pregFlags); }
}
class RecursiveRegexIterator extends RegexIterator implements RecursiveIterator {
    public function __construct(RecursiveIterator $iterator, string $pattern, int $mode = RecursiveRegexIterator::MATCH, int $flags = 0, int $pregFlags = 0) {
        __SplIt::once($this, 'RecursiveRegexIterator');
        if ($mode < 0 || $mode >= 5) { __SplIt::regexModeError('RecursiveRegexIterator::__construct', 3); }
        __SplIt::compileRegex($pattern, 'RecursiveRegexIterator::__construct');
        __SplIt::construct($this, $iterator, ['it' => $iterator, 'regex' => $pattern, 'mode' => $mode, 'flags' => $flags, 'pflags' => $pregFlags]);
    }
    public function accept(): bool {
        if (!__SplIt::get($this, 'has')) { return false; }
        $data = __SplIt::get($this, 'data');
        if (is_array($data)) { return count($data) > 0; }
        return __SplIt::regexAccept($this);
    }
    public function hasChildren(): bool { return __SplIt::get($this, 'inner')->hasChildren(); }
    public function getChildren(): RecursiveRegexIterator {
        $children = __SplIt::get($this, 'inner')->getChildren();
        return new static($children, __SplIt::get($this, 'regex'), __SplIt::get($this, 'mode'), __SplIt::get($this, 'flags'), __SplIt::get($this, 'pflags'));
    }
}
class RecursiveIteratorIterator implements OuterIterator {
    const LEAVES_ONLY = 0;
    const SELF_FIRST = 1;
    const CHILD_FIRST = 2;
    const CATCH_GET_CHILD = 16;
    public function __construct(Traversable $iterator, int $mode = RecursiveIteratorIterator::LEAVES_ONLY, int $flags = 0) { __SplIt::riConstruct($this, $iterator, $mode, $flags); }
    public function rewind(): void { __SplIt::riRewind($this); }
    public function valid(): bool { return __SplIt::riValid($this); }
    public function key(): mixed { return __SplIt::riLevelIt($this)->key(); }
    public function current(): mixed { return __SplIt::riCurrent($this); }
    public function next(): void { __SplIt::riForward($this); }
    public function getDepth(): int { return __SplIt::riDepth($this); }
    public function getSubIterator(?int $level = null): ?RecursiveIterator { return __SplIt::riSubIterator($this, $level); }
    public function getInnerIterator(): RecursiveIterator { return __SplIt::riLevelIt($this); }
    public function beginIteration(): void {}
    public function endIteration(): void {}
    public function callHasChildren(): bool { return __SplIt::riHasChildren($this); }
    public function callGetChildren(): ?RecursiveIterator { return __SplIt::riLevelIt($this)->getChildren(); }
    public function beginChildren(): void {}
    public function endChildren(): void {}
    public function nextElement(): void {}
    public function setMaxDepth(int $maxDepth = -1): void { __SplIt::riSetMax($this, $maxDepth); }
    public function getMaxDepth(): int|false { return __SplIt::riGetMax($this); }
    public function __call($name, $args) { return __SplIt::riForward_method($this, $name, $args); }
}
class RecursiveTreeIterator extends RecursiveIteratorIterator {
    const BYPASS_CURRENT = 4;
    const BYPASS_KEY = 8;
    const PREFIX_LEFT = 0;
    const PREFIX_MID_HAS_NEXT = 1;
    const PREFIX_MID_LAST = 2;
    const PREFIX_END_HAS_NEXT = 3;
    const PREFIX_END_LAST = 4;
    const PREFIX_RIGHT = 5;
    public function __construct(RecursiveIterator|IteratorAggregate $iterator, int $flags = RecursiveTreeIterator::BYPASS_KEY, int $cachingIteratorFlags = CachingIterator::CATCH_GET_CHILD, int $mode = RecursiveTreeIterator::SELF_FIRST) {
        if ($iterator instanceof IteratorAggregate) { $iterator = __SplIt::fromAggregate($iterator, get_class($iterator)); }
        __SplIt::riConstruct($this, new RecursiveCachingIterator($iterator, $cachingIteratorFlags), $mode, $flags);
    }
    public function key(): mixed {
        $key = __SplIt::riLevelIt($this)->key();
        if (__SplIt::get($this, 'flags') & 8) { return $key; }
        $key = (string) $key;
        return __SplIt::treePrefix($this) . $key . __SplIt::treeParts($this)[1];
    }
    public function current(): mixed {
        __SplIt::id($this);
        if (__SplIt::get($this, 'flags') & 4) { return __SplIt::riCurrent($this); }
        $entry = __SplIt::treeEntry($this);
        return __SplIt::treePrefix($this) . $entry . __SplIt::treeParts($this)[1];
    }
    public function getPrefix(): string { return __SplIt::treePrefix($this); }
    public function setPostfix(string $postfix): void { __SplIt::treeSetPostfix($this, $postfix); }
    public function setPrefixPart(int $part, string $value): void { __SplIt::treeSetPart($this, $part, $value); }
    public function getEntry(): string { __SplIt::id($this); return __SplIt::treeEntry($this); }
    public function getPostfix(): string { __SplIt::id($this); return __SplIt::treeParts($this)[1]; }
}
// ext/spl/spl_array.c's RecursiveArrayIterator: an array or object element
// has children, which are iterated by an iterator of this same class.
class RecursiveArrayIterator extends ArrayIterator implements RecursiveIterator {
    const CHILD_ARRAYS_ONLY = 4;
    public function hasChildren(): bool {
        if (!parent::valid()) { return false; }
        $e = parent::current();
        return is_array($e) || (is_object($e) && (parent::getFlags() & self::CHILD_ARRAYS_ONLY) === 0);
    }
    public function getChildren(): ?RecursiveArrayIterator {
        if (!parent::valid()) { return null; }
        $e = parent::current();
        if (is_object($e)) {
            if (parent::getFlags() & self::CHILD_ARRAYS_ONLY) { return null; }
            if (is_a($e, get_class($this))) { return $e; }
        }
        return new static($e, parent::getFlags());
    }
}
"#;

/// The function-table key the `Closure` a `Closure::fromCallable($c)` builds
/// runs out of: a variadic forwarder holding the callable it was made from.
///
/// Spelled with a leading `@` so no PHP program can declare, call, or see a
/// function by this name — the same convention the compiler's own synthetic
/// closure definitions use — and with a name of its own rather than the
/// `@closureN` the snippet below compiles to, because a user program's Nth
/// closure would otherwise be merged over it.
pub const FROM_CALLABLE_FORWARDER: &str = "@__from_callable";

/// The body of that forwarder, as PHP: everything the closure is given goes to
/// the captured callable, positional and named alike — which is exactly what
/// `f(...)` already lowers to, so the two forms of "a Closure over a callable"
/// forward identically.
const FROM_CALLABLE_SRC: &str = r#"<?php $f = fn(...$args) => call_user_func_array($c, $args);"#;

/// A compiled program's installable definitions: `(functions, classes)`.
type PreludeDefs = (Vec<(String, host::FuncDef)>, Vec<(String, host::ClassDef)>);

/// The compiled prelude's functions and classes, built once and merged onto every
/// fresh host before the user program (so user declarations of the same name win).
fn prelude_defs() -> &'static PreludeDefs {
    use std::sync::OnceLock;
    static CACHE: OnceLock<PreludeDefs> = OnceLock::new();
    CACHE.get_or_init(|| {
        let src = format!(
            "{EXCEPTION_PRELUDE}{DATETIME_PRELUDE}{HASH_PRELUDE}{WEAKREF_PRELUDE}{SPL_PRELUDE}{REFLECTION_PRELUDE}"
        );
        // NOT `compile`: that applies the file-level facts of whatever it is given,
        // and this runs from `load_merged` — after the user program was compiled —
        // so going through it would reset the typing mode the user's own
        // `declare(strict_types=1)` had just set. The prelude declares nothing of
        // the sort, so it has no facts of its own to apply.
        let stmts = parser::parse(&src).expect("prelude parses");
        let prog = compiler::compile_prelude(&stmts).expect("prelude compiles");
        let mut functions = prog.functions;
        // The forwarder is compiled from its own snippet — the closure literal
        // is the only definition it produces — and re-keyed to a name of its
        // own before it joins the table.
        let snippet = parser::parse(FROM_CALLABLE_SRC).expect("forwarder parses");
        let fwd = compiler::compile(&snippet, false).expect("forwarder compiles");
        let (_, def) = fwd
            .functions
            .into_iter()
            .next()
            .expect("the forwarder snippet defines one closure");
        functions.push((FROM_CALLABLE_FORWARDER.to_string(), def));
        let types = prog
            .classes
            .iter()
            .map(|(lname, def)| {
                let kind = if def.is_trait {
                    "trait"
                } else if def.is_interface {
                    "interface"
                } else if def.is_enum {
                    "enum"
                } else {
                    "class"
                };
                (lname.clone(), (def.name.clone(), kind))
            })
            .collect();
        let _ = PRELUDE_TYPES.set(types);
        (functions, prog.classes)
    })
}

/// The prelude's types by lowercased name: `(declared spelling, kind word)`.
/// Filled once the prelude has compiled, so it is empty WHILE the prelude
/// compiles — which is what keeps the prelude from colliding with itself.
static PRELUDE_TYPES: std::sync::OnceLock<rustc_hash::FxHashMap<String, (String, &'static str)>> =
    std::sync::OnceLock::new();

/// A type the PHP-written prelude declares, as a redeclaration names it:
/// `(declared spelling, kind word)`. Built-in to a PHP program, so a user
/// declaration of the same name is refused as it would be for any other
/// built-in type.
pub(crate) fn prelude_type(lname: &str) -> Option<(String, &'static str)> {
    PRELUDE_TYPES.get()?.get(lname).cloned()
}

/// The classes the PHP-written prelude declares, keyed by lowercased name.
/// The prelude is merged onto the host only once the user program has
/// compiled, so a check made WHILE compiling (a class linking against
/// `Exception`) reads the definitions from here.
///
/// Compiles the prelude on first use, so it must not be reached while the
/// prelude itself is compiling, nor from inside `host::with_host`.
pub(crate) fn prelude_classes() -> &'static [(String, host::ClassDef)] {
    &prelude_defs().1
}

/// Merge an already-compiled program onto the current host (install the exception
/// prelude, then the program's user functions/classes/try-defs) and return the
/// main chunk for the caller to run.
pub fn load_merged(prog: compiler::Program) -> fusevm::Chunk {
    let compiler::Program {
        main,
        main_locals,
        main_order,
        main_promoted,
        functions,
        classes,
        try_defs,
        diags: _,
        counters,
        fn_sites,
        class_sites,
    } = prog;
    let (prelude_fns, prelude_classes) = prelude_defs();
    host::with_host(|h| {
        // Prelude first, then the user program — a user redeclaration wins.
        h.load_program(prelude_fns.clone());
        h.load_classes(prelude_classes.clone());
        h.load_program(functions);
        h.load_classes(classes);
        h.load_try_defs(try_defs);
        let file = h.script_name().to_string();
        h.record_decl_sites(&file, fn_sites, class_sites);
        // A later `include` or `eval` continues this program's numbering.
        h.set_compile_counters(counters);
        // Reserve the global frame's slots before the chunk that numbered them
        // runs. An `include`/`eval` later in the same frame keeps the by-name
        // path, which reaches these same slots.
        h.seed_global_slots(&main_locals);
        h.set_main_layout(main_order, main_promoted);
    });
    main
}

/// Run an already-compiled program on the current host.
///
/// Compile-time notices are flushed first, ahead of the program's own output —
/// the reference engine finishes reading the whole source before it executes a
/// line of it, so `echo "a"; $v = 1; echo "${v}";` prints the deprecation notice
/// BEFORE the `a`, not between the two statements.
///
/// The output buffers still open at the end are finalized (their handlers run
/// with `PHP_OUTPUT_HANDLER_FINAL`), as the end of a request does. A whole
/// program defers that to `run_program`'s shutdown sequence, after the
/// shutdown functions and destructors, whose output the buffers still take.
pub fn run_compiled(prog: compiler::Program) -> Result<Value, String> {
    let r = run_compiled_open(prog);
    crate::stdlib::output::end_all();
    r
}

/// [`run_compiled`] leaving the output buffers open.
fn run_compiled_open(mut prog: compiler::Program) -> Result<Value, String> {
    let diags = std::mem::take(&mut prog.diags);
    let main = load_merged(prog);
    if !diags.is_empty() {
        host::with_host(|h| {
            for d in &diags {
                h.diagnose(d.severity, d.level, d.line, &d.msg);
            }
        });
    }
    host::run_main(main)
}

/// Run a WHOLE program: [`run_compiled`] with the output buffers left open,
/// then the request shutdown that frees what the program left behind —
/// shutdown functions, destructors, suspended generators, and last the
/// output buffers (see [`host::request_shutdown`] and
/// [`host::shutdown_generators`]). The REPL runs each line through
/// [`run_compiled`] alone, since its variables outlive the line.
///
/// Shutdown functions run after ANY fatal the runtime displayed; destructors
/// only after a clean end, an `exit`, or an uncaught exception — a fatal
/// error proper marks every object destructed in the reference.
fn run_program(prog: compiler::Program) -> Result<Value, String> {
    let r = run_compiled_open(prog);
    let destructors = match &r {
        Ok(_) => true,
        Err(e) => e.starts_with("Fatal error:  Uncaught "),
    };
    let shut = if r.is_ok() || host::fatal_reported() {
        host::request_shutdown(destructors)
    } else {
        Ok(())
    };
    host::shutdown_generators();
    match (r, shut) {
        (Err(e), _) | (Ok(_), Err(e)) => Err(e),
        (Ok(v), Ok(())) => Ok(v),
    }
}

/// Parse, compile, load, and run a PHP source string on a fresh host; return the
/// value of the last top-level expression.
pub fn eval_str(src: &str) -> Result<Value, String> {
    host::reset_host();
    run_program(compile(src)?)
}

/// Compile `src`, displaying a syntax error the way the PHP CLI does: a
/// `Parse error: …` copy on stdout and a `PHP `-prefixed copy on stderr. The
/// error is still returned so the caller can take an exit status from it, with
/// [`host::fatal_reported`] marking that it has already been shown.
///
/// Only the *parser*'s failures are framed this way. A later compile failure has
/// no reference equivalent (it is scaffold-specific), so it falls through to the
/// terse `php: …` form.
fn compile_cli(src: &str) -> Result<compiler::Program, String> {
    if let Err(e) = parser::parse_meta(src) {
        // A `declare(strict_types=…)` violation is a `Fatal error`, not a
        // `Parse error`; both stop the run before it produces any output.
        host::with_host(|h| h.fatal(e.severity, &e.message));
        return Err(e.message);
    }
    compile(src).map_err(|e| match e.strip_prefix(compiler::COMPILE_FATAL) {
        // A compile-time `Fatal error` the compiler raised (a redeclaration):
        // shown in PHP's shape, exactly as the parser's are.
        Some(body) => {
            host::with_host(|h| h.fatal("Fatal error", body));
            body.to_string()
        }
        None => e,
    })
}

/// [`eval_str`] for the CLI: identical, except a syntax error is displayed in
/// PHP's shape rather than left to the caller to print tersely, and the trailing
/// command-line arguments reach the program as `$argv`.
pub fn eval_cli(src: &str, args: &[String]) -> Result<Value, String> {
    host::reset_host();
    // `php -r` code names itself `Command line code` in diagnostics and
    // `__FILE__`, but `Standard input code` in `$argv[0]`. The reference really
    // does disagree with itself here, so the two are set from different values.
    host::with_host(|h| {
        h.set_script_args(None, args);
        h.disable_exception_handler();
    });
    run_program(compile_cli(src)?)
}

/// [`eval_cli`] for a script read from standard input (`php < script.php`).
///
/// The SAME source names itself differently under the three CLI entry points, so
/// the name cannot be inferred from the text: `php -r` code is `Command line
/// code`, a named file is its resolved path, and stdin is `Standard input code`.
/// Every diagnostic quotes it, and so does `__FILE__`.
pub fn eval_stdin_cli(src: &str, args: &[String]) -> Result<Value, String> {
    host::reset_host();
    host::with_host(|h| {
        h.set_script_name("Standard input code");
        h.set_script_args(None, args);
    });
    run_program(compile_cli(src)?)
}

/// [`eval_file`] for the CLI — see [`eval_cli`].
pub fn eval_file_cli(path: &str, args: &[String]) -> Result<Value, String> {
    let src = std::fs::read(path)
        .map(|b| crate::bytes::from_bytes(&b))
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    host::reset_host();
    set_script_name(path);
    // `$argv[0]` is the path AS WRITTEN on the command line, where the diagnostic
    // name just set is the resolved one — `php sub/s.php` reports both.
    host::with_host(|h| h.set_script_args(Some(path), args));
    run_program(compile_cli(&src)?)
}

/// Read and run a `.php` file on a fresh host.
pub fn eval_file(path: &str) -> Result<Value, String> {
    let src = std::fs::read(path)
        .map(|b| crate::bytes::from_bytes(&b))
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    host::reset_host();
    set_script_name(path);
    run_program(compile(&src)?)
}

/// Name this run's source for diagnostics. PHP prints the script's *resolved*
/// path in `Warning: … in <file> on line N`, so a relative argument is expanded;
/// a fresh host defaults to `"Command line code"`, which is what `php -r` uses.
fn set_script_name(path: &str) {
    let resolved = std::fs::canonicalize(path)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.to_string());
    host::with_host(|h| h.set_script_name(resolved));
}

/// Read and run a `.php` file under the DAP debugger (per-statement line markers,
/// tracing JIT disabled so the markers fire).
pub fn eval_file_debug(path: &str) -> Result<Value, String> {
    let src = std::fs::read(path)
        .map(|b| crate::bytes::from_bytes(&b))
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    let prog = compile_debug(&src)?;
    host::reset_host();
    set_script_name(path);
    host::set_debug_mode(true);
    let r = run_compiled(prog);
    host::set_debug_mode(false);
    r
}

/// Evaluate `src` with `vars` bound and return the captured program output —
/// [`eval_capture`] for an embedder that has input to hand the program.
///
/// The variables are seeded *after* the host reset that starts every run, which
/// is the whole reason this exists: setting them beforehand through
/// `host::with_host` cannot work, because the reset wipes them. Each is bound as
/// an ordinary PHP variable, so `("stdin", "…")` reads as `$stdin`.
///
/// ```no_run
/// let out = phplang::eval_capture_with("<?php echo strtoupper($stdin);", &[("stdin", "hi")]);
/// assert_eq!(out.unwrap(), "HI");
/// ```
pub fn eval_capture_with(src: &str, vars: &[(&str, &str)]) -> Result<String, String> {
    host::reset_host();
    host::with_host(|h| {
        for (name, text) in vars {
            h.set_var(name, Value::Str(std::sync::Arc::new(text.to_string())));
        }
        h.begin_capture();
    });
    let r = run_compiled(compile(src)?);
    let out = host::with_host(|h| h.end_capture());
    r.map(|_| out)
}

/// Evaluate `src` and return the captured program output. The convenience entry
/// point for tests: installs an output buffer, runs, and returns what `echo`
/// wrote (PHP is output-oriented — its observable result is stdout, not a value).
pub fn eval_capture(src: &str) -> Result<String, String> {
    host::reset_host();
    host::with_host(|h| h.begin_capture());
    let r = run_compiled(compile(src)?);
    let out = host::with_host(|h| h.end_capture());
    r.map(|_| out)
}
