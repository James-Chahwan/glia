package shop

import akka.stream.alpakka.sqs.scaladsl.SqsSource
import software.amazon.awssdk.services.sqs.SqsAsyncClient

object OrderPoller {
  def source(implicit sqs: SqsAsyncClient) = SqsSource("https://sqs.us-east-1.amazonaws.com/123/orders")
}
