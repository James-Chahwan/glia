<?php

use Ratchet\Http\HttpServer;
use Ratchet\Server\IoServer;
use Ratchet\WebSocket\WsServer;
use Ratchet\App;

$app = new App('localhost', 8080);
$app->route('/ws/chat', new Chat(), ['*']);
$app->run();
