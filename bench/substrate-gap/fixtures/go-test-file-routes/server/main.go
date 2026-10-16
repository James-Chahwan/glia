package main

import "github.com/gin-gonic/gin"

func main() {
	r := gin.Default()
	Register(r)
	_ = r.Run()
}

func Register(r *gin.Engine) {
	api := r.Group("/api")
	api.GET("/orders", listOrders)
	api.POST("/orders", placeOrder)
	api.GET("/ab_test", abTest)
}

func listOrders(c *gin.Context) {}
func placeOrder(c *gin.Context) {}
func abTest(c *gin.Context)     {}
