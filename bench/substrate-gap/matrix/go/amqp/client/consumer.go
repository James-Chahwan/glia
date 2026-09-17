package main

import "github.com/streadway/amqp"

func ConsumeOrders(ch *amqp.Channel) error {
	q, err := ch.QueueDeclare("orders", true, false, false, false, nil)
	if err != nil {
		return err
	}
	msgs, err := ch.Consume(q.Name, "", true, false, false, false, nil)
	if err != nil {
		return err
	}
	for d := range msgs {
		handle(d.Body)
	}
	return nil
}

func handle(b []byte) {}
