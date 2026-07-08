<?php

namespace App\Services;

class Greeter
{
    public function greet(string $name): string
    {
        return "Hello " . $name;
    }
}
