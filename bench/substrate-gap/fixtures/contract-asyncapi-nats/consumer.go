package orders

import "github.com/nats-io/nats.go"

func SubscribeOrders(nc *nats.Conn) (*nats.Subscription, error) {
	return nc.Subscribe("orders", handle)
}

func handle(m *nats.Msg) {}
