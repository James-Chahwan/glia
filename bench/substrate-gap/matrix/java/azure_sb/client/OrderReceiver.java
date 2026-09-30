package com.example;

import com.azure.messaging.servicebus.ServiceBusClientBuilder;
import com.azure.messaging.servicebus.ServiceBusReceiverClient;

public class OrderReceiver {
    public ServiceBusReceiverClient build(String conn) {
        return new ServiceBusClientBuilder().connectionString(conn)
            .receiver().queueName("orders").buildClient();
    }
}
