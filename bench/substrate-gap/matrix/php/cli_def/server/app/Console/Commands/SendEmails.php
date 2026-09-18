<?php

namespace App\Console\Commands;

use Illuminate\Console\Command;

class SendEmails extends Command
{
    protected $signature = 'emails:send {user} {--queue}';

    public function handle()
    {
    }
}
