package main

import (
	"github.com/nats-io/nats.go"
)

// PublishOrder publishes onto the NATS "orders" subject, and PublishBackfill
// publishes onto the SAME subject from the SAME file — two call sites for one
// topic node, which is what the CODE cell's `sites` array records.
func PublishOrder(nc *nats.Conn, payload []byte) error {
	return nc.Publish("orders", payload)
}

func PublishBackfill(nc *nats.Conn, payload []byte) error {
	return nc.Publish("orders", payload)
}
