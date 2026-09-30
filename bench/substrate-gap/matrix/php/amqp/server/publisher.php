<?php

use PhpAmqpLib\Connection\AMQPStreamConnection;
use PhpAmqpLib\Message\AMQPMessage;

$conn = new AMQPStreamConnection('localhost', 5672, 'guest', 'guest');
$channel = $conn->channel();
$channel->basic_publish(new AMQPMessage('order-1'), '', 'orders');
