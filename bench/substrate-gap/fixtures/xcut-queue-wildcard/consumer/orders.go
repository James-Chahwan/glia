package main

import (
	"github.com/nats-io/nats.go"
)

// SubscribeOrders receives every single-token subject under "orders.".
func SubscribeOrders(nc *nats.Conn) (*nats.Subscription, error) {
	return nc.Subscribe("orders.*", func(m *nats.Msg) {
		handle(m.Data)
	})
}

func handle(data []byte) {}
