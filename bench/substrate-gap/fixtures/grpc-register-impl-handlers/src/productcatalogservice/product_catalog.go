package main

import (
	"context"

	pb "github.com/example/demo/genproto/hipstershop"
)

type productCatalog struct {
	pb.UnimplementedProductCatalogServiceServer
}

func (p *productCatalog) ListProducts(ctx context.Context, _ *pb.Empty) (*pb.ListProductsResponse, error) {
	return &pb.ListProductsResponse{}, nil
}

func (p *productCatalog) GetProduct(ctx context.Context, req *pb.GetProductRequest) (*pb.Product, error) {
	return &pb.Product{}, nil
}
