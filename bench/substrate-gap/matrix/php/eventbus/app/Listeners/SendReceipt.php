<?php

namespace App\Listeners;

use App\Events\OrderPlaced;

class SendReceipt
{
    public function handle(OrderPlaced $event): void {}
}

function place(int $id): void
{
    OrderPlaced::dispatch($id);
}
