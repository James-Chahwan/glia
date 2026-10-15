<?php
namespace App\Orders;

use \App\Models\{User, Order};

class Checkout
{
    public function run(int $id): void
    {
        User::find($id);
        Order::create($id);
    }
}
