from kafka import KafkaConsumer


def run_billing():
    consumer = KafkaConsumer("orders.created", bootstrap_servers="kafka:9092")
    for message in consumer:
        record(message.value)


def record(order):
    return order
