package main

import (
	"context"
	"log"

	pb "example.com/proto/user"
	"google.golang.org/grpc"
)

// FetchUser is the gateway's gRPC client for api/user.proto (GRPC_CALLS).
func FetchUser(id string) {
	conn, err := grpc.Dial("localhost:50051", grpc.WithInsecure())
	if err != nil {
		log.Fatal(err)
	}
	defer conn.Close()
	client := pb.NewUserServiceClient(conn)
	resp, _ := client.GetUser(context.Background(), &pb.GetUserRequest{Id: id})
	log.Println(resp)
}
