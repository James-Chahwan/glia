package consumer

import (
	"context"

	"github.com/segmentio/kafka-go"
)

// Consume reads from the topic named in the ReaderConfig struct literal; the
// receiver on the read call is `r`, not `reader`.
func Consume(ctx context.Context) error {
	r := kafka.NewReader(kafka.ReaderConfig{
		Brokers: []string{"localhost:9092"},
		Topic:   "orders",
		GroupID: "svc",
	})
	for {
		m, err := r.ReadMessage(ctx)
		if err != nil {
			return err
		}
		_ = m
	}
}
