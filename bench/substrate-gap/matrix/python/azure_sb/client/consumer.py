import os

from azure.servicebus import ServiceBusClient

client = ServiceBusClient.from_connection_string(os.environ["SERVICEBUS_CONNECTION_STR"])


def run():
    with client.get_queue_receiver(queue_name="orders") as receiver:
        for msg in receiver.receive_messages(max_message_count=10, max_wait_time=5):
            handle(str(msg))
            receiver.complete_message(msg)


def handle(body):
    print(body)
