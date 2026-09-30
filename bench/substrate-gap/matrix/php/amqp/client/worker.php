<?php

use PhpAmqpLib\Connection\AMQPStreamConnection;

$conn = new AMQPStreamConnection('localhost', 5672, 'guest', 'guest');
$channel = $conn->channel();
$channel->queue_declare('orders', false, true, false, false);
$channel->basic_consume('orders', '', false, true, false, false, function ($msg) {
    echo $msg->body;
});
