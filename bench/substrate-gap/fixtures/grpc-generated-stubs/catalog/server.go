package main

import (
	"context"
	"net"

	pb "example.com/catalog/genproto"
	"google.golang.org/grpc"
)

type catalog struct {
	pb.UnimplementedCatalogServiceServer
}

func (c *catalog) ListProducts(ctx context.Context, _ *pb.Empty) (*pb.ListProductsResponse, error) {
	return &pb.ListProductsResponse{}, nil
}

func main() {
	lis, err := net.Listen("tcp", ":3550")
	if err != nil {
		return
	}
	srv := grpc.NewServer()
	pb.RegisterCatalogServiceServer(srv, &catalog{})
	_ = srv.Serve(lis)
}
