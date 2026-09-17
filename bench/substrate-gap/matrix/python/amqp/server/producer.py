import pika

connection = pika.BlockingConnection(pika.ConnectionParameters("localhost"))
channel = connection.channel()
channel.queue_declare(queue="orders")


def publish_order(body):
    channel.basic_publish(exchange="", routing_key="orders", body=body)
