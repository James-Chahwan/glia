package main

import (
	"github.com/acme/nats.go"
	"github.com/acme/nats.go/internal/v2store"
)

func main() {
	nats.Connect("x")
	store.Open()
}
