package main

import (
	"context"
	"log"

	"google.golang.org/grpc"
)

func FetchUser(id string) {
	conn, err := grpc.Dial("users-svc:50051", grpc.WithInsecure())
	if err != nil {
		log.Fatal(err)
	}
	defer conn.Close()
	client := NewUserServiceClient(conn)
	_, _ = client.GetUser(context.Background(), nil)
}
