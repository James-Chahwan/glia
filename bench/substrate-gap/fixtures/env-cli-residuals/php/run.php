<?php

function thumbnail(PDO $db): void
{
    exec('convert a.png b.jpg');
    $db->exec('CREATE TABLE thumbs (id INTEGER)');
    $p = new \Symfony\Component\Process\Process(['git', 'status']);
    $p->run();
}
