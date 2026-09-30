<?php

use PhpMqtt\Client\MqttClient;

$mqtt = new MqttClient('broker', 1883);
$mqtt->connect();
$mqtt->publish('sensors/temp', '21', 0);
