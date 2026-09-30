package graph

import (
	"context"

	"example.com/shop/graph/model"
)

func (r *queryResolver) Orders(ctx context.Context) ([]*model.Order, error) {
	return nil, nil
}
