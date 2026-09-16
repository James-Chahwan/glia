package main

import (
	"github.com/nats-io/nats.go"
)

// ConsumeOrders subscribes to the NATS "orders" subject.
func ConsumeOrders(nc *nats.Conn) (*nats.Subscription, error) {
	return nc.Subscribe("orders", func(m *nats.Msg) {
		handle(m.Data)
	})
}

func handle(data []byte) {}
