from kafka import KafkaProducer

producer = KafkaProducer(bootstrap_servers="kafka:9092")


def reissue_order(order):
    producer.send("orders.created", order)
