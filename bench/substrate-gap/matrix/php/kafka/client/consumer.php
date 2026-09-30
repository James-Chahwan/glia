<?php

$conf = new RdKafka\Conf();
$consumer = new RdKafka\KafkaConsumer($conf);
$consumer->subscribe(['orders']);
$message = $consumer->consume(120 * 1000);
