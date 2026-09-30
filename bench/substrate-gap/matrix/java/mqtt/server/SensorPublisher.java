package com.example;

import org.eclipse.paho.client.mqttv3.MqttClient;
import org.eclipse.paho.client.mqttv3.MqttMessage;

public class SensorPublisher {
    public void send(MqttClient client) throws Exception {
        client.publish("sensors/temp", new MqttMessage("21".getBytes()));
    }
}
