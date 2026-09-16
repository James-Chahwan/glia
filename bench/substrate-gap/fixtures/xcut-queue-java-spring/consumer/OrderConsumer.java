package com.shop.orders;

import java.time.Duration;
import java.util.Arrays;

import org.apache.kafka.clients.consumer.ConsumerRecord;
import org.apache.kafka.clients.consumer.ConsumerRecords;
import org.apache.kafka.clients.consumer.KafkaConsumer;

public class OrderConsumer {

    public void run(KafkaConsumer<String, String> consumer) {
        consumer.subscribe(Arrays.asList("orders"));
        ConsumerRecords<String, String> records = consumer.poll(Duration.ofMillis(100));
        for (ConsumerRecord<String, String> record : records) {
            handle(record.value());
        }
    }

    private void handle(String value) {
    }
}
