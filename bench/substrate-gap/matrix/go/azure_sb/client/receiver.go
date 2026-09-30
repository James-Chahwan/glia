package main

import (
	"context"

	"github.com/Azure/azure-sdk-for-go/sdk/messaging/azservicebus"
)

func Drain(ctx context.Context, client *azservicebus.Client) error {
	receiver, err := client.NewReceiverForQueue("orders", nil)
	if err != nil {
		return err
	}
	_, err = receiver.ReceiveMessages(ctx, 10, nil)
	return err
}
