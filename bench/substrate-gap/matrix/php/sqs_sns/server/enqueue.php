<?php

use Aws\Sqs\SqsClient;

$sqs = new SqsClient(['region' => 'us-east-1', 'version' => 'latest']);
$sqs->sendMessage(['QueueUrl' => 'https://sqs.us-east-1.amazonaws.com/123/orders', 'MessageBody' => 'order-1']);
