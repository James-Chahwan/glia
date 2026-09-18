<?php

namespace App\Billing;

class Invoice
{
    public function total()
    {
        return $this->round();
    }

    private function round()
    {
        return 1;
    }
}

interface Payable
{
    public function pay();
}
