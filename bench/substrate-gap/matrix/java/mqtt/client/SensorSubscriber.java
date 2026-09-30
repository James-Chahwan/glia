package com.example;

import org.eclipse.paho.client.mqttv3.MqttClient;

public class SensorSubscriber {
    public void listen(MqttClient client) throws Exception {
        client.subscribe("sensors/temp", (topic, msg) -> System.out.println(msg));
    }
}
