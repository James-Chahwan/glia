package shop

import akka.stream.alpakka.amqp.{AmqpConnectionProvider, AmqpWriteSettings}
import akka.stream.alpakka.amqp.scaladsl.AmqpSink

object OrderPublisher {
  def sink(conn: AmqpConnectionProvider) =
    AmqpSink.simple(AmqpWriteSettings(conn).withRoutingKey("orders"))
}
