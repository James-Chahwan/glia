package producer

import (
	"github.com/nats-io/nats.go"
	amqp "github.com/rabbitmq/amqp091-go"
)

// Bridge copies order events from RabbitMQ onto NATS.
type Bridge struct {
	ch   *amqp.Channel
	conn *nats.Conn
}

// Relay publishes one body to both brokers.
func (b *Bridge) Relay(body []byte) {
	_ = b.ch.Publish("shop", "orders.relay", false, false, amqp.Publishing{Body: body})
	_ = b.conn.Publish("shop.orders.relayed", body)
}
