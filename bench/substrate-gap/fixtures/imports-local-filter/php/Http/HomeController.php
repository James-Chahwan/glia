<?php
namespace App\Http;

use App\Services\Greeter;
use GuzzleHttp\Client;

class HomeController
{
    public function index(): string
    {
        return (new Greeter())->greet();
    }
}
