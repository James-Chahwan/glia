import paho.mqtt.client as mqtt

client = mqtt.Client()
client.connect("broker", 1883)


def send_telemetry(payload):
    client.publish("sensors/temp", payload)
