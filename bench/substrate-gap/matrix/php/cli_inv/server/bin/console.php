<?php

use Symfony\Component\Console\Application;
use Symfony\Component\Console\Attribute\AsCommand;
use Symfony\Component\Console\Command\Command;

#[AsCommand(name: 'sync', description: 'Sync records')]
class SyncCommand extends Command {}

$app = new Application('mytool');
$app->add(new SyncCommand());
$app->run();
