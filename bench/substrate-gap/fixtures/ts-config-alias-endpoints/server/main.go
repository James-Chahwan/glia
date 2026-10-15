package main

import "github.com/gin-gonic/gin"

func main() {
	r := gin.Default()
	api := r.Group("/api")
	api.GET("/trades", trades)
	api.GET("/notifications/unread", unread)
	admin := r.Group("/admin")
	admin.GET("/stats", stats)
	admin.GET("/users", users)
	r.GET("/stats", publicStats)
	_ = r.Run()
}

func trades(c *gin.Context)      {}
func unread(c *gin.Context)      {}
func stats(c *gin.Context)       {}
func publicStats(c *gin.Context) {}
func users(c *gin.Context)       {}
