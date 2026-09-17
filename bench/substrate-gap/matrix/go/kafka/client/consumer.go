package main

import (
	"context"

	"github.com/segmentio/kafka-go"
)

func ConsumeOrders(ctx context.Context) error {
	reader := kafka.NewReader(kafka.ReaderConfig{Brokers: []string{"localhost:9092"}, Topic: "orders", GroupID: "workers"})
	for {
		m, err := reader.ReadMessage(ctx)
		if err != nil {
			return err
		}
		handle(m.Value)
	}
}

func handle(v []byte) {}
