package shop

import akka.stream.alpakka.amqp.{AmqpConnectionProvider, NamedQueueSourceSettings}
import akka.stream.alpakka.amqp.scaladsl.AmqpSource

object OrderConsumer {
  def source(conn: AmqpConnectionProvider) =
    AmqpSource.atMostOnceSource(NamedQueueSourceSettings(conn, "orders"), bufferSize = 10)
}
