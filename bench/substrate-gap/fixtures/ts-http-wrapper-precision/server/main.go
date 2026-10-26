package main

import (
	"github.com/gin-gonic/gin"
)

func list(c *gin.Context)  {}
func stats(c *gin.Context) {}

func main() {
	r := gin.Default()
	r.GET("/api/items", list)
	r.GET("/api/stats", stats)
	r.Run(":8080")
}
