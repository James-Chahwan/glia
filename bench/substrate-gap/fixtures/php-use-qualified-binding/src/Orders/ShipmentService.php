<?php
namespace App\Orders;

use App\Shipping\Invoicer;

class ShipmentService
{
    public function ship(int $id): void
    {
        Invoicer::issue($id);
    }
}
