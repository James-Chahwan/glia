<?php

use PhpAmqpLib\Connection\AMQPStreamConnection;
use PhpAmqpLib\Message\AMQPMessage;

$conn = new AMQPStreamConnection('localhost', 5672, 'guest', 'guest');
$channel = $conn->channel();
// $channel->basic_publish($m, '', 'ghost_php');
$doc = '$channel->basic_publish($m, "", "ghost_php");';
$msg = new AMQPMessage('invoice-1');
$channel->basic_publish($msg, '', 'invoices');
