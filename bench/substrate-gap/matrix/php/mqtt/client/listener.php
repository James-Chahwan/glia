<?php

use PhpMqtt\Client\MqttClient;

$mqtt = new MqttClient('broker', 1883);
$mqtt->connect();
$mqtt->subscribe('sensors/temp', function ($topic, $message) {
    echo $message;
}, 0);
$mqtt->loop(true);
