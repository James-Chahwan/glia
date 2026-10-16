package graph

import (
	"context"

	"example.com/shop/api/graph/model"
)

func (r *queryResolver) Products(ctx context.Context, first *int) ([]*model.Product, error) {
	return []*model.Product{}, nil
}

func (r *queryResolver) ProductByID(ctx context.Context, id string) (*model.Product, error) {
	return &model.Product{ID: id}, nil
}

func (r *mutationResolver) Restock(ctx context.Context, id string, qty int) (*model.Product, error) {
	r.Stock[id] += qty
	return &model.Product{ID: id, Stock: r.Stock[id]}, nil
}

func (r *Resolver) Query() QueryResolver       { return &queryResolver{r} }
func (r *Resolver) Mutation() MutationResolver { return &mutationResolver{r} }

type queryResolver struct{ *Resolver }
type mutationResolver struct{ *Resolver }

type QueryResolver interface{}
type MutationResolver interface{}
