<?php

use Basis\Nats\Client;

$client = new Client();
$client->subscribe('orders', function ($payload) {
    echo $payload;
});
$client->process();
