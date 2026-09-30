<?php

use LaunchDarkly\LDClient;
use LaunchDarkly\LDContext;

function variant(LDClient $client, string $userKey): string
{
    return $client->variation('new-checkout', LDContext::create($userKey), false) ? 'new' : 'legacy';
}
