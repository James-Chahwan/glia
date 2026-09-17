package main

import (
	"context"

	"github.com/segmentio/kafka-go"
)

var writer = &kafka.Writer{
	Addr:  kafka.TCP("localhost:9092"),
	Topic: "orders",
}

func PublishOrder(ctx context.Context, payload []byte) error {
	return writer.WriteMessages(ctx, kafka.Message{Value: payload})
}
