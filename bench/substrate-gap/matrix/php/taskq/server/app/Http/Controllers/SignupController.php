<?php

namespace App\Http\Controllers;

use App\Jobs\SendWelcomeEmail;

class SignupController
{
    public function store()
    {
        SendWelcomeEmail::dispatch();
    }
}
