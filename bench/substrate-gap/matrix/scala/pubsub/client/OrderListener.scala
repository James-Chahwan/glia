package shop

import akka.stream.alpakka.googlecloud.pubsub.PubSubConfig
import akka.stream.alpakka.googlecloud.pubsub.scaladsl.GooglePubSub

object OrderListener {
  def source(config: PubSubConfig) = GooglePubSub.subscribe("orders", config)
}
