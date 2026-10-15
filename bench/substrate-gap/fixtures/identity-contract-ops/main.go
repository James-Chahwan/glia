package main

import "github.com/gin-gonic/gin"

// @title Shop API
func main() {
	r := gin.Default()
	api := r.Group("/api")
	api.GET("/orders", listOrders)
	r.Run()
}

// @Summary List orders
// @Router /orders [get]
func listOrders(c *gin.Context) {
	c.JSON(200, []string{})
}
