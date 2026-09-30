package com.example;

import com.azure.messaging.servicebus.ServiceBusClientBuilder;
import com.azure.messaging.servicebus.ServiceBusMessage;
import com.azure.messaging.servicebus.ServiceBusSenderClient;

public class OrderSender {
    public void send(String conn, String body) {
        ServiceBusSenderClient sender = new ServiceBusClientBuilder().connectionString(conn)
            .sender().queueName("orders").buildClient();
        sender.sendMessage(new ServiceBusMessage(body));
    }
}
