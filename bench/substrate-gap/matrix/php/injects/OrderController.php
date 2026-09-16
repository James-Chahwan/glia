<?php

namespace App\Http\Controllers;

use App\Services\UserService;

class OrderController
{
    public function __construct(private UserService $users)
    {
    }
}
