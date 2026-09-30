<?php

use Shop\OrderServiceClient;
use Shop\OrderRequest;

$client = new OrderServiceClient('localhost:50051', ['credentials' => Grpc\ChannelCredentials::createInsecure()]);
[$reply, $status] = $client->GetOrder(new OrderRequest())->wait();
