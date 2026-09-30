package main

import (
	"example.com/shop/api"
	"github.com/gin-gonic/gin"
)

func main() {
	r := gin.Default()
	v2 := r.Group("/api/v2")
	api.RegisterUsers(v2)
	api.RegisterUsers(r.Group("/api/v1"))
	s := api.NewServer(r)
	_ = s
	r.Run()
}
