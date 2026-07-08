package main

import (
	"github.com/nats-io/nats.go"
)

// OrderWorker subscribes to the NATS "orders" subject and handles events.
func SubscribeOrders(nc *nats.Conn) (*nats.Subscription, error) {
	return nc.Subscribe("orders", func(m *nats.Msg) {
		handle(m.Data)
	})
}

func handle(data []byte) {}
