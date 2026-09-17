package main

import (
	mqtt "github.com/eclipse/paho.mqtt.golang"
)

func Listen(client mqtt.Client) {
	token := client.Subscribe("sensors/temp", 0, func(c mqtt.Client, m mqtt.Message) {
		handle(m.Payload())
	})
	token.Wait()
}

func handle(b []byte) {}
