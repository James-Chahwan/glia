package main

import "github.com/gin-gonic/gin"

func listFriends(c *gin.Context)  {}
func acceptFriend(c *gin.Context) {}

func main() {
	r := gin.Default()
	api := r.Group("/api")
	api.GET("/protected/friends", listFriends)
	api.POST("/protected/friends/accept/:publicId", acceptFriend)
	r.Run(":8080")
}
