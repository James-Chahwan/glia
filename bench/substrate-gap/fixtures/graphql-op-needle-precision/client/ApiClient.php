<?php
class ApiClient
{
    public function users()
    {
        return $this->http->request('GET', '/users');
    }
}
