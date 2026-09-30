<?php

use Basis\Nats\Client;

$client = new Client();
$client->publish('orders', 'order-1');
