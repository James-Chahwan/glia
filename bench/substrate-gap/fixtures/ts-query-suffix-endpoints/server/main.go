package main

import "github.com/gin-gonic/gin"

func main() {
	r := gin.Default()
	api := r.Group("/api")
	api.GET("/orders", listOrders)
	api.GET("/orders/:id", getOrder)
	api.GET("/products", listProducts)
	admin := r.Group("/admin")
	admin.GET("/smr", listSmr)
	admin.POST("/smr", createSmr)
	_ = r.Run()
}

func listOrders(c *gin.Context)   {}
func getOrder(c *gin.Context)     {}
func listProducts(c *gin.Context) {}
func listSmr(c *gin.Context)      {}
func createSmr(c *gin.Context)    {}
