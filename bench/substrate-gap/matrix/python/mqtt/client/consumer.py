import paho.mqtt.client as mqtt


def on_message(client, userdata, msg):
    print(msg.topic, msg.payload)


client = mqtt.Client()
client.on_message = on_message
client.connect("broker", 1883)
client.subscribe("sensors/temp")
client.loop_forever()
