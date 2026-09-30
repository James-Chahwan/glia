<?php
namespace App\Orders {
use App\Billing\Ledger;

class ShipmentService
{
    public function ship(int $id): void
    {
        Ledger::issue($id);
    }
}
}
