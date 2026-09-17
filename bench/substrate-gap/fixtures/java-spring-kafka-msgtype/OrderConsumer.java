package com.demo.worker;

import org.apache.kafka.clients.consumer.ConsumerRecord;
import org.springframework.kafka.annotation.KafkaListener;
import org.springframework.stereotype.Component;

@Component
public class OrderConsumer {

    @KafkaListener(topics = "orders")
    public void handle(ConsumerRecord<String, OrderCreated> record) {
        System.out.println(record.value());
    }
}
