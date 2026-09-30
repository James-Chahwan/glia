<?php

use Illuminate\Support\Facades\Redis;

Redis::publish('orders', json_encode(['id' => 1]));
