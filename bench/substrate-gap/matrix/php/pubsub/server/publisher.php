<?php

use Google\Cloud\PubSub\PubSubClient;

$pubsub = new PubSubClient(['projectId' => 'shop']);
$pubsub->topic('orders')->publish(['data' => 'order-1']);
