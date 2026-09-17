import boto3

sqs = boto3.client("sqs")


def poll():
    resp = sqs.receive_message(
        QueueUrl="https://sqs.us-east-1.amazonaws.com/123456789012/orders",
        MaxNumberOfMessages=10,
    )
    for msg in resp.get("Messages", []):
        handle(msg["Body"])


def handle(body):
    print(body)
