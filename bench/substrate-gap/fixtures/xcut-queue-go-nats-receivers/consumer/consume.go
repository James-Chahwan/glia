package consumer

import (
	"github.com/nats-io/nats.go"
)

// Service drains the shop's order subjects.
type Service struct {
	JS nats.JetStreamContext
}

// Start listens for shipped orders.
func Start(conn *nats.Conn) {
	conn.Subscribe("shop.orders.shipped", func(m *nats.Msg) {})
}

// Listen hands cancelled orders to onCancel.
func (s *Service) Listen(conn *nats.Conn) {
	conn.Subscribe("shop.orders.cancelled", s.onCancel)
}

func (s *Service) onCancel(m *nats.Msg) {}

// Drain pulls a backlog from a subject named at runtime.
func (s *Service) Drain(subject string) {
	s.JS.PullSubscribe(subject, "", nats.ManualAck())
}
