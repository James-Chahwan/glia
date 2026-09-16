<?php

namespace App\Http\Controllers;

class UserController extends Controller
{
    public function show($id)
    {
        return User::find($id);
    }

    public function store()
    {
        return "stored";
    }

    public function stats()
    {
        return [];
    }
}
