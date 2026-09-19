package main

import "github.com/nats-io/nats.go"

func onOrder(m *nats.Msg) {}

func onAudit(m *nats.Msg) { record(m) }

func record(m *nats.Msg) {}

type Worker struct{ nc *nats.Conn }

func (w *Worker) handle(m *nats.Msg) {}

func (w *Worker) Start() {
	w.nc.Subscribe("refunds", w.handle)
}

func main() {
	nc, _ := nats.Connect(nats.DefaultURL)
	nc.Subscribe("orders", onOrder)
	nc.QueueSubscribe("audit", "workers", func(m *nats.Msg) { onAudit(m) })
}
