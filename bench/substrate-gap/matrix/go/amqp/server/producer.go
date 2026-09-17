package main

import "github.com/streadway/amqp"

func PublishOrder(ch *amqp.Channel, body []byte) error {
	return ch.Publish("", "orders", false, false, amqp.Publishing{
		ContentType: "application/json",
		Body:        body,
	})
}
