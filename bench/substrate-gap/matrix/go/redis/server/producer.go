package main

import (
	"context"

	"github.com/redis/go-redis/v9"
)

func PublishOrder(ctx context.Context, rdb *redis.Client, payload string) error {
	return rdb.LPush(ctx, "orders", payload).Err()
}
