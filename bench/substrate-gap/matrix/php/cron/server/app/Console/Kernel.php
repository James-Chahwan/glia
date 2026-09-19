<?php

namespace App\Console;

use App\Jobs\Heartbeat;
use Illuminate\Console\Scheduling\Schedule;
use Illuminate\Foundation\Console\Kernel as ConsoleKernel;

class Kernel extends ConsoleKernel
{
    protected function schedule(Schedule $schedule)
    {
        $schedule->command('emails:send')->daily();
        $schedule->job(new Heartbeat)->everyFiveMinutes();
        $schedule->command('reports:build')->cron('15 1 * * 1');
    }
}
