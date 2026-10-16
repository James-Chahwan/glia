package graph

import (
	"context"

	"example.com/stock/api/graph/model"
	"example.com/stock/api/internal/stock"
)

func toModel(it *stock.Item) *model.Item {
	return &model.Item{ID: it.ID, Label: it.Label, PriceCents: int(it.PriceCents), OnHand: it.OnHand}
}

func (r *queryResolver) Items(ctx context.Context, first *int) ([]*model.Item, error) {
	var out []*model.Item
	for _, it := range r.Warehouse.All() {
		out = append(out, toModel(it))
	}
	return out, nil
}

func (r *mutationResolver) AdjustStock(ctx context.Context, id string, delta int) (*model.Item, error) {
	return toModel(r.Warehouse.Adjust(id, delta)), nil
}

func (r *Resolver) Query() QueryResolver { return &queryResolver{r} }

func (r *Resolver) Mutation() MutationResolver { return &mutationResolver{r} }

type queryResolver struct{ *Resolver }

type mutationResolver struct{ *Resolver }
