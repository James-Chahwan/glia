package shop

import akka.stream.alpakka.googlecloud.pubsub.PubSubConfig
import akka.stream.alpakka.googlecloud.pubsub.scaladsl.GooglePubSub

object OrderPublisher {
  def flow(config: PubSubConfig) = GooglePubSub.publish("orders", config)
}
