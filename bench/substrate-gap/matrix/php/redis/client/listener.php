<?php

use Illuminate\Support\Facades\Redis;

Redis::subscribe(['orders'], function (string $message) {
    echo $message;
});
