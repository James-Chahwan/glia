package shop

import akka.stream.alpakka.sqs.scaladsl.SqsPublishSink
import software.amazon.awssdk.services.sqs.SqsAsyncClient

object OrderEnqueuer {
  def sink(implicit sqs: SqsAsyncClient) = SqsPublishSink("https://sqs.us-east-1.amazonaws.com/123/orders")
}
