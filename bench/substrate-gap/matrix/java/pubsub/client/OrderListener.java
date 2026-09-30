package com.example;

import com.google.cloud.spring.pubsub.core.PubSubTemplate;

public class OrderListener {
    public OrderListener(PubSubTemplate pubSubTemplate) {
        pubSubTemplate.subscribe("orders", message -> message.ack());
    }
}
