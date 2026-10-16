package producer

import (
	"github.com/nats-io/nats.go"
)

// Bus is an in-process fan-out that happens to hold a NATS connection.
type Bus struct {
	nc *nats.Conn
}

// Publish delivers v to local listeners only; it never touches NATS.
func (b *Bus) Publish(topic string, v any) {}

// Local raises an in-process event.
func Local(bus *Bus) {
	bus.Publish("local.only", nil)
}
