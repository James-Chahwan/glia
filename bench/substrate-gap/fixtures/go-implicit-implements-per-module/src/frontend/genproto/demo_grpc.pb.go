package hipstershop

import (
	"context"

	grpc "google.golang.org/grpc"
)

type CartServiceClient interface {
	AddItem(ctx context.Context, in *AddItemRequest, opts ...grpc.CallOption) (*Empty, error)
	GetCart(ctx context.Context, in *GetCartRequest, opts ...grpc.CallOption) (*Cart, error)
}

type cartServiceClient struct {
	cc grpc.ClientConnInterface
}

func NewCartServiceClient(cc grpc.ClientConnInterface) CartServiceClient {
	return &cartServiceClient{cc}
}

func (c *cartServiceClient) AddItem(ctx context.Context, in *AddItemRequest, opts ...grpc.CallOption) (*Empty, error) {
	return new(Empty), nil
}

func (c *cartServiceClient) GetCart(ctx context.Context, in *GetCartRequest, opts ...grpc.CallOption) (*Cart, error) {
	return new(Cart), nil
}

type Closer interface {
	Close() error
}
