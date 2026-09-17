package com.shop.workers;

import org.springframework.amqp.core.Message;
import org.springframework.amqp.rabbit.annotation.RabbitListener;
import org.springframework.stereotype.Component;

@Component
public class OrderListener {
    @RabbitListener(queues = "orders")
    public void onOrder(Message message) {
        System.out.println(new String(message.getBody()));
    }
}
