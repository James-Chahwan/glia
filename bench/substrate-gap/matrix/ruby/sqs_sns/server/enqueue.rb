require "aws-sdk-sqs"

sqs = Aws::SQS::Client.new
sqs.send_message(queue_url: "https://sqs.us-east-1.amazonaws.com/123/orders", message_body: "order-1")
