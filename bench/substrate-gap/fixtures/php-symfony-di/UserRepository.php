<?php

namespace App\Repository;

class UserRepository
{
    public function find(int $id): array
    {
        return ['id' => $id];
    }
}
