package com.example;

import io.nats.client.Connection;
import io.nats.client.Message;
import io.nats.client.Nats;
import io.nats.client.Subscription;

public class OrderSubscriber {
    public Message next() throws Exception {
        Connection nc = Nats.connect("nats://localhost:4222");
        Subscription sub = nc.subscribe("orders");
        return sub.nextMessage(java.time.Duration.ofSeconds(1));
    }
}
