package main

import (
	"context"

	"github.com/redis/go-redis/v9"
)

func ConsumeOrders(ctx context.Context, rdb *redis.Client) error {
	for {
		res, err := rdb.BLPop(ctx, 0, "orders").Result()
		if err != nil {
			return err
		}
		handle(res[1])
	}
}

func handle(v string) {}
