package com.shop.messaging;

import software.amazon.awssdk.services.sqs.SqsClient;
import software.amazon.awssdk.services.sqs.model.SendMessageRequest;

public class OrderProducer {
    private final SqsClient sqs = SqsClient.create();

    public void publish(String payload) {
        sqs.sendMessage(SendMessageRequest.builder()
                .queueUrl("https://sqs.us-east-1.amazonaws.com/123456789012/orders")
                .messageBody(payload)
                .build());
    }
}
