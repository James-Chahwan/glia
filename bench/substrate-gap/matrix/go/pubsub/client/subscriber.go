package main

import (
	"context"

	"cloud.google.com/go/pubsub"
)

func Consume(ctx context.Context, client *pubsub.Client) error {
	sub := client.Subscription("orders")
	return sub.Receive(ctx, func(ctx context.Context, m *pubsub.Message) {
		m.Ack()
	})
}
