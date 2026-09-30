package shop

import io.nats.client.Nats

object OrderPublisher {
  def publish(body: Array[Byte]): Unit = {
    val nc = Nats.connect("nats://localhost:4222")
    nc.publish("orders", body)
  }
}
