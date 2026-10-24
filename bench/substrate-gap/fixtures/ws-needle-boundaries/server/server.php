<?php

use Ratchet\App;

$app = new App('localhost', 8080);
$app->route("/rooms/$room", new Chat, ['*']);
$app->run();
