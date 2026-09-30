require "aws-sdk-sqs"

sqs = Aws::SQS::Client.new
sqs.receive_message(queue_url: "https://sqs.us-east-1.amazonaws.com/123/orders", max_number_of_messages: 10)
