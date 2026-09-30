<?php

namespace App;

class EloquentUserRepository implements UserRepository
{
    public function find(int $id): array
    {
        return ['id' => $id];
    }
}
