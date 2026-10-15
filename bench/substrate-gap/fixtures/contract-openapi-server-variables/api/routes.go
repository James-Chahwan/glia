package api

import "github.com/gin-gonic/gin"

func Register(r *gin.Engine) {
	v1 := r.Group("/api/v1")
	v1.GET("/users", listUsers)
	r.GET("/admin/stats", stats)
}

func listUsers(c *gin.Context) {}
func stats(c *gin.Context)     {}
