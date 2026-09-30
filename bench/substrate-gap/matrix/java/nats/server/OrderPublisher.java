package com.example;

import io.nats.client.Connection;
import io.nats.client.Nats;

public class OrderPublisher {
    public void publish(byte[] body) throws Exception {
        Connection nc = Nats.connect("nats://localhost:4222");
        nc.publish("orders", body);
    }
}
