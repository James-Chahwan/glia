package producer

import (
	"context"

	"github.com/segmentio/kafka-go"
)

// Publish writes one message. The receiver is `w`, not `writer` — the old
// needle table was bound to the receiver NAME and saw nothing here.
func Publish(ctx context.Context, w *kafka.Writer, body []byte) error {
	return w.WriteMessages(ctx, kafka.Message{Topic: "orders", Value: body})
}
