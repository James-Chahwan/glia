package main

import (
	"context"

	pb "example.com/frontend/genproto"
	"google.golang.org/grpc"
)

func getCart(ctx context.Context, conn *grpc.ClientConn, userID string) (*pb.Cart, error) {
	return pb.NewCartServiceClient(conn).GetCart(ctx, &pb.GetCartRequest{UserId: userID})
}

func listProducts(ctx context.Context, conn *grpc.ClientConn) (*pb.ListProductsResponse, error) {
	return pb.NewCatalogServiceClient(conn).ListProducts(ctx, &pb.Empty{})
}
