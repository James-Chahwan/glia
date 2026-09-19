package main

import (
	"github.com/google/uuid"
	"example.com/svc/internal/store"
	"example.com/svc-b/client"
)

func main() {
	_ = uuid.New()
	store.Save()
	client.Get()
}
