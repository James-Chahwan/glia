package producer

import (
	"github.com/nats-io/nats.go"
)

// Publisher announces order events on the shop bus.
type Publisher struct {
	conn *nats.Conn
}

// OrderShipped publishes the shipped event for one order.
func (p *Publisher) OrderShipped(id string) error {
	return p.conn.Publish("shop.orders.shipped", []byte(id))
}

// Forward relays a payload to a subject chosen by the caller.
func Forward(conn *nats.Conn, subject string, b []byte) error {
	return conn.Publish(subject, b)
}
