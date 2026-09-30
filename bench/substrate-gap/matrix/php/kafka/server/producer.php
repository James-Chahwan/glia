<?php

$producer = new RdKafka\Producer(new RdKafka\Conf());
$topic = $producer->newTopic("orders");
$topic->produce(RD_KAFKA_PARTITION_UA, 0, "order-1");
