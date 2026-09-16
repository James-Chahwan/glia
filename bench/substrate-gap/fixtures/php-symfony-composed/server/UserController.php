<?php

namespace App\Controller;

use Symfony\Component\Routing\Annotation\Route;

#[Route('/api/v1/users')]
class UserController
{
    #[Route('/{id}', methods: ['GET'])]
    public function show(int $id)
    {
        return $id;
    }

    #[Route('', methods: ['POST'])]
    public function create()
    {
        return "created";
    }
}
