<?php

namespace App\Services;

class UserService
{
    public function find(int $id): string
    {
        return "user-{$id}";
    }
}
