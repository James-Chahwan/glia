<?php

namespace App\Controller;

use App\Repository\UserRepository;

class UserController
{
    public function __construct(private readonly UserRepository $users, private int $pageSize = 20)
    {
    }

    public function show(int $id): array
    {
        return $this->users->find($id);
    }
}
