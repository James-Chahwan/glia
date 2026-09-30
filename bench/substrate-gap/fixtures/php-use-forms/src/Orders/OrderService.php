<?php
namespace App\Orders;

use App\Billing\{Invoicer, Ledger as Book};
use function App\Util\money_format;
use \App\Models\Customer;

class OrderService
{
    public function place(int $id): string
    {
        Invoicer::issue($id);
        Book::post($id);
        Customer::find($id);
        return money_format($id);
    }
}
