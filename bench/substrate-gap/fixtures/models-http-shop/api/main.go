package main

import (
	"github.com/gin-gonic/gin"

	"example.com/billing/api/handlers"
	"example.com/billing/api/store"
)

func main() {
	r := gin.Default()
	api := r.Group("/api")
	handlers.New(store.New()).Register(api)
	_ = r.Run(":8080")
}
