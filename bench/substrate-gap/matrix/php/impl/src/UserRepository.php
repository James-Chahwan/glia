<?php

namespace App;

interface UserRepository
{
    public function find(int $id): array;
}
