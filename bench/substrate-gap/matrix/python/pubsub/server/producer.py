from google.cloud import pubsub_v1

publisher = pubsub_v1.PublisherClient()
topic_path = publisher.topic_path("my-project", "orders")


def publish_order(data):
    future = publisher.publish(topic_path, data)
    return future.result()
