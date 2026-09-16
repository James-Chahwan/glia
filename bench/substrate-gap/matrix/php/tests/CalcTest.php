<?php

namespace App\Tests;

use App\Calc;
use PHPUnit\Framework\TestCase;

class CalcTest extends TestCase
{
    public function testAdd(): void
    {
        $this->assertSame(5, (new Calc())->add(2, 3));
    }
}
