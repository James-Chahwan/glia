<?php

use Google\Cloud\PubSub\PubSubClient;

$pubsub = new PubSubClient(['projectId' => 'shop']);
$subscription = $pubsub->subscription('orders');
foreach ($subscription->pull() as $message) {
    $subscription->acknowledge($message);
}
