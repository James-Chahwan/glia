package main

import (
	"github.com/nats-io/nats.go"
)

// SubscribeBilling receives "billing.*" only — never an orders subject.
func SubscribeBilling(nc *nats.Conn) (*nats.Subscription, error) {
	return nc.Subscribe("billing.*", func(m *nats.Msg) {
		handle(m.Data)
	})
}
