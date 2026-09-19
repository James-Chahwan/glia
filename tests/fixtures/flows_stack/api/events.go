package main

import "github.com/nats-io/nats.go"

func onCreated(m *nats.Msg) {}

func subscribe(nc *nats.Conn) {
	nc.Subscribe("orders.created", onCreated)
}
