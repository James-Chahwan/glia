package shop

import com.azure.messaging.servicebus.{ServiceBusClientBuilder, ServiceBusMessage}

object OrderSender {
  def send(conn: String, body: String): Unit =
    new ServiceBusClientBuilder().connectionString(conn).sender().queueName("orders").buildClient().sendMessage(new ServiceBusMessage(body))
}
