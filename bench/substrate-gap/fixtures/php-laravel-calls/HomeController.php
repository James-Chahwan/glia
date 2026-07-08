<?php

namespace App\Http\Controllers;

use App\Services\Greeter;

class HomeController extends Controller
{
    public function index()
    {
        $greeter = new Greeter();
        return $greeter->greet("world");
    }

    public function show()
    {
        return $this->index();
    }
}
