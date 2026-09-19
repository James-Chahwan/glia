package main

import (
	"context"
	"log"

	"google.golang.org/grpc"
	pb "example.com/proto/user"
)

func FetchUser(id string) {
	conn, err := grpc.Dial("users-svc:50051", grpc.WithInsecure())
	if err != nil {
		log.Fatal(err)
	}
	defer conn.Close()

	client := pb.NewUserServiceClient(conn)
	resp, err := client.GetUser(context.Background(), &pb.GetUserRequest{Id: id})
	if err != nil {
		log.Fatal(err)
	}
	log.Println(resp)
}
