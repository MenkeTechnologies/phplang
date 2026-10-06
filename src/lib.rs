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
pub mod cli;
pub mod compiler;
pub mod corpus;
pub mod dap;
pub mod errlevel;
pub mod host;
pub mod intercepts;
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
/// `throw` site. DIVERGENCE: `getTrace()` (the structured array form) is not
/// provided — only the rendered `getTraceAsString()`.
const EXCEPTION_PRELUDE: &str = r#"<?php
class Exception {
    protected $message = "";
    protected $code = 0;
    protected $previous = null;
    protected $file = "";
    protected $line = 0;
    protected $trace = "";
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
    final public function getTraceAsString() { return $this->trace; }
    public function __toString() { return __phplang_throwable_string($this); }
}
class Error {
    protected $message = "";
    protected $code = 0;
    protected $previous = null;
    protected $file = "";
    protected $line = 0;
    protected $trace = "";
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
    final public function getTraceAsString() { return $this->trace; }
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
class ArrayObject implements IteratorAggregate, ArrayAccess, Countable {
    const STD_PROP_LIST = 1;
    const ARRAY_AS_PROPS = 2;
    private $storage = [];
    public function __construct(array|object $array = [], int $flags = 0, string $iteratorClass = ArrayIterator::class) { $this->storage = is_array($array) ? $array : get_object_vars($array); }
    public function offsetGet(mixed $key): mixed { return $this->storage[$key]; }
    public function offsetSet(mixed $key, mixed $value): void { if ($key === null) { $this->storage[] = $value; } else { $this->storage[$key] = $value; } }
    public function offsetExists(mixed $key): bool { return isset($this->storage[$key]); }
    public function offsetUnset(mixed $key): void { unset($this->storage[$key]); }
    public function append(mixed $value): void { $this->storage[] = $value; }
    public function count(): int { return count($this->storage); }
    public function getArrayCopy(): array { return $this->storage; }
    public function exchangeArray(array|object $array): array { $old = $this->storage; $this->storage = is_array($array) ? $array : get_object_vars($array); return $old; }
    public function getIterator(): Iterator { return new ArrayIterator($this->storage); }
    public function getFlags(): int { return 0; }
    public function setFlags(int $flags): void {}
    public function getIteratorClass(): string { return ArrayIterator::class; }
    public function asort(int $flags = SORT_REGULAR): bool { return asort($this->storage, $flags); }
    public function ksort(int $flags = SORT_REGULAR): bool { return ksort($this->storage, $flags); }
    public function uasort(callable $callback): bool { return uasort($this->storage, $callback); }
    public function uksort(callable $callback): bool { return uksort($this->storage, $callback); }
    public function natsort(): bool { return natsort($this->storage); }
    public function natcasesort(): bool { return natcasesort($this->storage); }
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
final class ReturnTypeWillChange { public function __construct() {} }
final class AllowDynamicProperties { public function __construct() {} }
final class SensitiveParameter { public function __construct() {} }
final class Override { public function __construct() {} }
// An iterator over an array, positioned by a cursor kept outside the
// instance (a static table keyed by object id) so the instance shows only
// its storage, as the reference's does.
class ArrayIterator implements SeekableIterator, ArrayAccess, Countable {
    private $storage = [];
    private static $__cursor = [];
    public function __construct(array|object $array = [], int $flags = 0) { $this->storage = is_array($array) ? $array : get_object_vars($array); }
    private function __at() {
        $c = self::$__cursor[spl_object_id($this)] ?? [0, null];
        if ($c[1] === null) { $c[1] = array_keys($this->storage); self::$__cursor[spl_object_id($this)] = $c; }
        return $c;
    }
    public function current(): mixed { [$i, $keys] = $this->__at(); return $i < count($keys) ? $this->storage[$keys[$i]] : null; }
    public function key(): string|int|null { [$i, $keys] = $this->__at(); return $keys[$i] ?? null; }
    public function next(): void { [$i, $keys] = $this->__at(); self::$__cursor[spl_object_id($this)] = [$i + 1, $keys]; }
    public function rewind(): void { self::$__cursor[spl_object_id($this)] = [0, array_keys($this->storage)]; }
    public function valid(): bool { [$i, $keys] = $this->__at(); return $i < count($keys); }
    public function seek(int $offset): void {
        if ($offset < 0 || $offset >= count($this->storage)) { throw new OutOfBoundsException("Seek position $offset is out of range"); }
        self::$__cursor[spl_object_id($this)] = [$offset, array_keys($this->storage)];
    }
    public function offsetGet(mixed $key): mixed { return $this->storage[$key]; }
    public function offsetSet(mixed $key, mixed $value): void { if ($key === null) { $this->storage[] = $value; } else { $this->storage[$key] = $value; } self::$__cursor[spl_object_id($this)][1] = null; }
    public function offsetExists(mixed $key): bool { return isset($this->storage[$key]); }
    public function offsetUnset(mixed $key): void { unset($this->storage[$key]); self::$__cursor[spl_object_id($this)][1] = null; }
    public function append(mixed $value): void { $this->storage[] = $value; self::$__cursor[spl_object_id($this)][1] = null; }
    public function count(): int { return count($this->storage); }
    public function getArrayCopy(): array { return $this->storage; }
    public function getFlags(): int { return 0; }
    public function setFlags(int $flags): void {}
    public function asort(int $flags = SORT_REGULAR): bool { return asort($this->storage, $flags); }
    public function ksort(int $flags = SORT_REGULAR): bool { return ksort($this->storage, $flags); }
    public function uasort(callable $callback): bool { return uasort($this->storage, $callback); }
    public function uksort(callable $callback): bool { return uksort($this->storage, $callback); }
    public function natsort(): bool { return natsort($this->storage); }
    public function natcasesort(): bool { return natcasesort($this->storage); }
}
class SplObjectStorage implements ArrayAccess, Countable {
    public $store = [];
    public function attach($obj, $data = null) { $this->store[spl_object_id($obj)] = $data; }
    public function detach($obj) { unset($this->store[spl_object_id($obj)]); }
    public function contains($obj) { return array_key_exists(spl_object_id($obj), $this->store); }
    public function count() { return count($this->store); }
    public function offsetGet($obj) { return $this->store[spl_object_id($obj)] ?? null; }
    public function offsetSet($obj, $data) { $this->store[spl_object_id($obj)] = $data; }
    public function offsetExists($obj) { return array_key_exists(spl_object_id($obj), $this->store); }
    public function offsetUnset($obj) { unset($this->store[spl_object_id($obj)]); }
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
        let src = format!("{EXCEPTION_PRELUDE}{DATETIME_PRELUDE}{SPL_PRELUDE}");
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
pub fn run_compiled(mut prog: compiler::Program) -> Result<Value, String> {
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

/// Run a WHOLE program: [`run_compiled`], then the request shutdown that frees
/// what the program left behind — shutdown functions, destructors and
/// suspended generators (see [`host::request_shutdown`] and
/// [`host::shutdown_generators`]). The REPL runs each line through
/// [`run_compiled`] alone, since its variables outlive the line.
///
/// Shutdown functions run after ANY fatal the runtime displayed; destructors
/// only after a clean end, an `exit`, or an uncaught exception — a fatal
/// error proper marks every object destructed in the reference.
fn run_program(prog: compiler::Program) -> Result<Value, String> {
    let r = run_compiled(prog);
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
    let src = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    host::reset_host();
    set_script_name(path);
    // `$argv[0]` is the path AS WRITTEN on the command line, where the diagnostic
    // name just set is the resolved one — `php sub/s.php` reports both.
    host::with_host(|h| h.set_script_args(Some(path), args));
    run_program(compile_cli(&src)?)
}

/// Read and run a `.php` file on a fresh host.
pub fn eval_file(path: &str) -> Result<Value, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
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
    let src = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
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
