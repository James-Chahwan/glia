<?php

use Symfony\Component\Process\Process;

$process = new Process(['mytool', 'sync']);
$process->run();
