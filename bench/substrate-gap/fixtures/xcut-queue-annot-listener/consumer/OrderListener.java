package com.shop.billing;

import io.awspring.cloud.sqs.annotation.SqsListener;
import org.springframework.kafka.annotation.KafkaListener;
import org.springframework.stereotype.Component;

@Component
public class OrderListener {

    @KafkaListener(topics = "orders", groupId = "billing")
    public void onOrder(String payload) {
        charge(payload);
    }

    @SqsListener("refunds")
    public void onRefund(String orderId) {
        refund(orderId);
    }

    private void charge(String payload) {
    }

    private void refund(String orderId) {
    }
}
