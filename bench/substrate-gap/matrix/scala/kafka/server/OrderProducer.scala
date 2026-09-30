package shop

import akka.kafka.ProducerSettings
import akka.kafka.scaladsl.Producer
import org.apache.kafka.clients.producer.ProducerRecord

object OrderProducer {
  def record(body: String) = new ProducerRecord[String, String]("orders", body)
}
