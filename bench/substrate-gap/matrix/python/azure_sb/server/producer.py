import os

from azure.servicebus import ServiceBusClient, ServiceBusMessage

client = ServiceBusClient.from_connection_string(os.environ["SERVICEBUS_CONNECTION_STR"])


def publish_order(body):
    with client.get_queue_sender(queue_name="orders") as sender:
        sender.send_messages(ServiceBusMessage(body))
