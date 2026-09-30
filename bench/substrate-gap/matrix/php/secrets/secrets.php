<?php

use Aws\SecretsManager\SecretsManagerClient;

$client = new SecretsManagerClient(['region' => 'us-east-1', 'version' => 'latest']);
$result = $client->getSecretValue(['SecretId' => 'prod/db-password']);
