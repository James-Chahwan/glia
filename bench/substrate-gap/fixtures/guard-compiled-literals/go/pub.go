package pub

import "github.com/nats-io/nats.go"

// js.Publish("ghost.go", b)

const doc = `
nc.Publish("ghost.raw", b)
`

func Send(nc *nats.Conn, b []byte) error {
	return nc.Publish("orders.go", b)
}
