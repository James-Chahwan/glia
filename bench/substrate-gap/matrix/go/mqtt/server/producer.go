package main

import (
	mqtt "github.com/eclipse/paho.mqtt.golang"
)

func SendTelemetry(client mqtt.Client, payload []byte) {
	token := client.Publish("sensors/temp", 0, false, payload)
	token.Wait()
}
