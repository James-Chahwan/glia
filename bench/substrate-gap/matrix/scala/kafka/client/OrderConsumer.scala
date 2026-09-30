package shop

import akka.kafka.{ConsumerSettings, Subscriptions}
import akka.kafka.scaladsl.Consumer

object OrderConsumer {
  def source(settings: ConsumerSettings[String, String]) =
    Consumer.plainSource(settings, Subscriptions.topics("orders"))
}
