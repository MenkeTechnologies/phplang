<?php
// Classes: properties with defaults, a constructor, methods, and $this.
class Animal {
    public $name;
    public $legs = 4;
    function __construct($name) {
        $this->name = $name;
    }
    function describe() {
        return $this->name . " has " . $this->legs . " legs";
    }
    function speak() {
        return "...";
    }
}

// Single inheritance: Dog overrides speak() and reuses the parent constructor.
class Dog extends Animal {
    const SOUND = "woof";
    function speak() {
        return $this->name . " says " . self::SOUND;
    }
}

$a = new Animal("Camel");
$a->legs = 4;
echo $a->describe(), "\n";

$d = new Dog("Rex");
echo $d->speak(), "\n";
echo $d->describe(), "\n";      // inherited method
echo Dog::SOUND, "\n";          // class constant
echo Dog::class, "\n";          // class-name string

// Constructor property promotion.
class Point {
    function __construct(public $x, public $y) {}
    function sum() {
        return $this->x + $this->y;
    }
}
$p = new Point(3, 4);
echo $p->sum(), "\n";
