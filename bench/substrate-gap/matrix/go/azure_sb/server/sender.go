package main

import (
	"context"

	"github.com/Azure/azure-sdk-for-go/sdk/messaging/azservicebus"
)

func Send(ctx context.Context, client *azservicebus.Client, body []byte) error {
	sender, err := client.NewSender("orders", nil)
	if err != nil {
		return err
	}
	return sender.SendMessage(ctx, &azservicebus.Message{Body: body}, nil)
}
