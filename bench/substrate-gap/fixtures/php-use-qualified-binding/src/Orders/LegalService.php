<?php
namespace App\Orders;

use App\Legal\Invoicer;

class LegalService
{
    public function check(int $id): void
    {
        Invoicer::issue($id);
    }
}
