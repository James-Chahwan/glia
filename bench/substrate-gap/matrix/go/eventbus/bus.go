package main

import evbus "github.com/asaskevich/EventBus"

func SendReceipt(orderID string) {}

func Wire() {
	bus := evbus.New()
	bus.Subscribe("order:placed", SendReceipt)
	bus.Publish("order:placed", "o-1")
}
