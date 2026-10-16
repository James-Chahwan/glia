package main

import (
	"log"

	"github.com/gin-gonic/gin"

	"github.com/example/shop/api/internal/handlers"
	"github.com/example/shop/api/internal/store"
)

func main() {
	st := store.NewMemory()

	r := gin.Default()
	api := r.Group("/api")
	handlers.NewOrders(st).Register(api)

	log.Fatal(r.Run(":8080"))
}
