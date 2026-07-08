package main

import (
	"github.com/nats-io/nats.go"
)

// OrderService publishes order events onto the NATS "orders" subject.
func PublishOrder(nc *nats.Conn, payload []byte) error {
	return nc.Publish("orders", payload)
}
