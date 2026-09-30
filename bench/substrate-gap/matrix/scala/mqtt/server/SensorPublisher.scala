package shop

import org.eclipse.paho.client.mqttv3.{MqttClient, MqttMessage}

object SensorPublisher {
  def send(client: MqttClient): Unit = client.publish("sensors/temp", new MqttMessage("21".getBytes))
}
