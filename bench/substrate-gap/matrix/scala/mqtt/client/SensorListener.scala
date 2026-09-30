package shop

import org.eclipse.paho.client.mqttv3.MqttClient

object SensorListener {
  def listen(client: MqttClient): Unit = client.subscribe("sensors/temp")
}
