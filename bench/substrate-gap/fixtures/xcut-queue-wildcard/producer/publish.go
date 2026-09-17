package main

import (
	"github.com/nats-io/nats.go"
)

// PublishCreated emits the concrete "orders.created" subject.
func PublishCreated(nc *nats.Conn, payload []byte) error {
	return nc.Publish("orders.created", payload)
}
