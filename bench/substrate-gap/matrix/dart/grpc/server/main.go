package main

import (
	"google.golang.org/grpc"

	pb "example.com/shop/orders"
)

type server struct{ pb.UnimplementedOrderServiceServer }

func main() {
	s := grpc.NewServer()
	pb.RegisterOrderServiceServer(s, &server{})
}
