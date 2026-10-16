package shop;

import org.springframework.kafka.annotation.KafkaListener;

public class OrderListener {
    @KafkaListener(topics = "orders")
    public void on(String body) {}
}
