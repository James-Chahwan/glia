package main

import (
	"github.com/gin-gonic/gin"
)

func listOrderUsers(c *gin.Context) {}

func getOrderUser(c *gin.Context) {}

func main() {
	r := gin.Default()
	r.GET("/users", listOrderUsers)
	r.GET("/users/:id", getOrderUser)
	r.Run(":8081")
}
