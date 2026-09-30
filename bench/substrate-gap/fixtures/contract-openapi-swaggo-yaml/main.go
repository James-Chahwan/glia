package main

import "github.com/gin-gonic/gin"

func main() {
	r := gin.Default()
	api := r.Group("/api")
	api.GET("/orders", listOrders)
	api.GET("/orders/:id", getOrder)
	r.Run()
}

func listOrders(c *gin.Context) {}
func getOrder(c *gin.Context)   {}
