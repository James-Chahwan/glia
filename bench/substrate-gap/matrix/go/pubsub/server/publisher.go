package main

import (
	"context"

	"cloud.google.com/go/pubsub"
)

func Publish(ctx context.Context, client *pubsub.Client, body []byte) error {
	topic := client.Topic("orders")
	_, err := topic.Publish(ctx, &pubsub.Message{Data: body}).Get(ctx)
	return err
}
