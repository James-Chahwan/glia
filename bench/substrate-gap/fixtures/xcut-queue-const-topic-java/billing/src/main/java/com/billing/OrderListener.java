package com.billing;

import org.springframework.kafka.annotation.KafkaListener;
import org.springframework.stereotype.Component;

@Component
public class OrderListener {

    @KafkaListener(topics = Topics.ORDERS, groupId = "billing")
    public void onOrder(String payload) {
    }
}
