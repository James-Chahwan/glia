from kafka import KafkaConsumer

consumer = KafkaConsumer("orders", bootstrap_servers="localhost:9092", group_id="workers")


def run():
    for msg in consumer:
        handle(msg.value)


def handle(value):
    print(value)
