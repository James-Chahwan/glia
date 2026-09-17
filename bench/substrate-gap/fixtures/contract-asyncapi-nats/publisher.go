package orders

import "github.com/nats-io/nats.go"

func PublishOrder(nc *nats.Conn, payload []byte) error {
	return nc.Publish("orders", payload)
}
