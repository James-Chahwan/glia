<?php

namespace App\Clients;

use GuzzleHttp\Client;

// Outbound HTTP client. Deliberately NOT under a routes/ path, and the receiver
// of every verb call here is a member chain (`$this->client`), never the bare
// `$app` variable the Slim router scanner keys on — so any ROUTE node emitted
// from this file is a phantom.
class ApiClient
{
    private $client;
    private $collection;

    // Guzzle shorthand verb + a concatenated `$id` tail -> GET /api/users/${…}.
    public function fetchUser($id)
    {
        return $this->client->get('/api/users/' . $id);
    }

    // Guzzle AND Symfony HttpClient share `request(VERB, URL, ...)`, so the
    // verb comes from the first argument -> POST /api/users.
    public function createUser($body)
    {
        return $this->client->request('POST', '/api/users', ['json' => $body]);
    }

    // NEGATIVE CONTROL: a verb-named call on a collection with a non-URL string
    // key — the case only `url_to_path` can reject.
    public function cached($id)
    {
        return $this->collection->get('user-' . $id);
    }
}
