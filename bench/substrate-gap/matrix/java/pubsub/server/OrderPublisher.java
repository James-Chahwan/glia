package com.example;

import com.google.cloud.spring.pubsub.core.PubSubTemplate;

public class OrderPublisher {
    private final PubSubTemplate pubSubTemplate;

    public OrderPublisher(PubSubTemplate pubSubTemplate) {
        this.pubSubTemplate = pubSubTemplate;
    }

    public void publish(String body) {
        pubSubTemplate.publish("orders", body);
    }
}
