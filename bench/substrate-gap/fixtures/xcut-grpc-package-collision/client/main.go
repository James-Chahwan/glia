package main

import (
	"context"

	"google.golang.org/grpc"

	pb "example.com/gen/billing"
)

func Charge(id string) {
	conn, _ := grpc.Dial("localhost:50051", grpc.WithInsecure())
	defer conn.Close()
	c := pb.NewPaymentsServiceClient(conn)
	c.Charge(context.Background(), &pb.ChargeRequest{Id: id})
}
