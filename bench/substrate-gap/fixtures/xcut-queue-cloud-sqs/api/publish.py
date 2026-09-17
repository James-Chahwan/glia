import boto3

sqs = boto3.client("sqs")


def publish_order(body):
    sqs.send_message(
        QueueUrl="https://sqs.us-east-1.amazonaws.com/123456789012/orders",
        MessageBody=body,
    )
