package shop

import com.azure.messaging.servicebus.ServiceBusClientBuilder

object OrderReceiver {
  def build(conn: String) = new ServiceBusClientBuilder().connectionString(conn).receiver().queueName("orders").buildClient()
}
