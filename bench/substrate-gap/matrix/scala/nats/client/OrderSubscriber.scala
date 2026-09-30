package shop

import io.nats.client.Nats

object OrderSubscriber {
  def next() = {
    val nc = Nats.connect("nats://localhost:4222")
    nc.subscribe("orders").nextMessage(java.time.Duration.ofSeconds(1))
  }
}
