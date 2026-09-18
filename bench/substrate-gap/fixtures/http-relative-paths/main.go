package main

import "github.com/gin-gonic/gin"

func listItems(c *gin.Context) {}

func main() {
	r := gin.Default()
	r.GET("items", listItems)
	r.Run()
}
