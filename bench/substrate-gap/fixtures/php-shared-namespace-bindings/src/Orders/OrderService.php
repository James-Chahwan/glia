<?php
namespace App\Orders {
use App\Billing\Invoicer;

class OrderService
{
    public function place(int $id): void
    {
        Invoicer::issue($id);
    }
}
}
