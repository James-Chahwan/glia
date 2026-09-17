import pika

connection = pika.BlockingConnection(pika.ConnectionParameters("localhost"))
channel = connection.channel()
channel.queue_declare(queue="orders")


def on_message(ch, method, properties, body):
    ch.basic_ack(delivery_tag=method.delivery_tag)


def run():
    channel.basic_consume(queue="orders", on_message_callback=on_message)
    channel.start_consuming()
