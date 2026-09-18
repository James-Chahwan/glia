<?php
namespace App\Command;

use Symfony\Component\Console\Attribute\AsCommand;
use Symfony\Component\Console\Command\Command;

#[AsCommand(name: 'app:sync-orders', description: 'Sync orders')]
class SyncCommand extends Command
{
}

class LegacyCommand extends Command
{
    protected static $defaultName = 'app:legacy-import';
}
