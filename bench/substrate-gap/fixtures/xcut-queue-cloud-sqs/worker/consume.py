import boto3

sqs = boto3.client("sqs")


def poll_orders():
    resp = sqs.receive_message(
        QueueUrl="https://sqs.us-east-1.amazonaws.com/123456789012/orders",
        MaxNumberOfMessages=10,
    )
    return resp.get("Messages", [])
