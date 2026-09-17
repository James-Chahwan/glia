from kafka import KafkaProducer

producer = KafkaProducer(bootstrap_servers="localhost:9092")


def publish_order(payload):
    producer.send("orders", payload)
    producer.flush()
