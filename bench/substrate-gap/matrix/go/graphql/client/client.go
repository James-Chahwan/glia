package main

import (
	"context"

	"github.com/machinebox/graphql"
)

func ListOrders(ctx context.Context) error {
	client := graphql.NewClient("http://api/query")
	req := graphql.NewRequest(`query { orders { id } }`)
	var resp map[string]any
	return client.Run(ctx, req, &resp)
}
