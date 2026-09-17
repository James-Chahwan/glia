package com.shop.workers;

import software.amazon.awssdk.services.sqs.SqsClient;
import software.amazon.awssdk.services.sqs.model.Message;
import software.amazon.awssdk.services.sqs.model.ReceiveMessageRequest;

public class OrderConsumer {
    private final SqsClient sqs = SqsClient.create();

    public void poll() {
        ReceiveMessageRequest request = ReceiveMessageRequest.builder()
                .queueUrl("https://sqs.us-east-1.amazonaws.com/123456789012/orders")
                .maxNumberOfMessages(10)
                .build();
        for (Message m : sqs.receiveMessage(request).messages()) {
            System.out.println(m.body());
        }
    }
}
