import pika

SAMPLE = "channel.basic_publish(exchange='', routing_key='refunds', body=b)"


def send_invoice(channel, body):
    channel.basic_publish(exchange='', routing_key='invoices', body=body)
