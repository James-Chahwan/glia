<?php

namespace App\Http\Controllers;

class UserController extends Controller
{
    public function index()
    {
        return User::all();
    }

    public function store()
    {
        return "stored";
    }
}
