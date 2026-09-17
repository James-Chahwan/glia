from google.cloud import pubsub_v1

subscriber = pubsub_v1.SubscriberClient()
subscription_path = subscriber.subscription_path("my-project", "orders-worker")


def callback(message):
    print(message.data)
    message.ack()


def run():
    streaming_pull = subscriber.subscribe(subscription_path, callback=callback)
    streaming_pull.result()
