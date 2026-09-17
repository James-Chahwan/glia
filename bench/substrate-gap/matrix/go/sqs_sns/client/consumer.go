package main

import (
	"context"
	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/service/sqs"
)

func PollOrders(ctx context.Context, client *sqs.Client) (bodies []string, err error) {
	out, err := client.ReceiveMessage(ctx, &sqs.ReceiveMessageInput{
		QueueUrl: aws.String("https://sqs.us-east-1.amazonaws.com/123456789012/orders"),
	})
	if err != nil {
		return nil, err
	}
	for _, m := range out.Messages {
		bodies = append(bodies, aws.ToString(m.Body))
	}
	return bodies, nil
}
