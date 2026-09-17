package com.shop.workers;

import org.apache.kafka.clients.consumer.ConsumerRecord;
import org.springframework.kafka.annotation.KafkaListener;
import org.springframework.stereotype.Component;

@Component
public class OrderListener {
    @KafkaListener(topics = "orders", groupId = "workers")
    public void onOrder(ConsumerRecord<String, String> record) {
        System.out.println(record.value());
    }
}
