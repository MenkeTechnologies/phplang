<?php
// Basic throw + catch, reading the message off the exception object.
try {
    throw new Exception("boom");
} catch (Exception $e) {
    echo "caught: ", $e->getMessage(), "\n";
}

// finally always runs — here after a normal try body.
try {
    echo "work";
} finally {
    echo " + cleanup\n";
}

// Exception and Error are disjoint roots: catch(Exception) does not catch a
// TypeError, so it falls through to catch(Error).
try {
    throw new TypeError("not an int");
} catch (Exception $e) {
    echo "as exception\n";
} catch (Error $e) {
    echo "as error: ", $e->getMessage(), "\n";
}

// A subclass is caught by its base class; multi-catch unions are supported.
try {
    throw new InvalidArgumentException("bad arg");
} catch (LogicException | RuntimeException $e) {
    echo "logic/runtime: ", $e->getMessage(), "\n";
}

// throw as a PHP 8 expression on the right of `??`.
function require_value($v) {
    return $v ?? throw new RuntimeException("missing");
}
try {
    require_value(null);
} catch (RuntimeException $e) {
    echo "required: ", $e->getMessage(), "\n";
}

// A user class can extend a built-in exception.
class ConfigError extends Exception {}
try {
    throw new ConfigError("bad config", 7);
} catch (Exception $e) {
    echo $e->getMessage(), " (", $e->getCode(), ")\n";
}
