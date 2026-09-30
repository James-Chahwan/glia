package api

import "github.com/gin-gonic/gin"

// RegisterUsers mounts the user routes on whatever group it is handed.
func RegisterUsers(rg *gin.RouterGroup) {
	rg.GET("/users", listUsers)
	me := rg.Group("/me")
	me.GET("/profile", profile)
}

// RegisterHealth is never called in this repo: its routes keep their path.
func RegisterHealth(rg *gin.RouterGroup) {
	rg.GET("/healthz", health)
}

func listUsers(c *gin.Context) {}
func profile(c *gin.Context)   {}
func health(c *gin.Context)    {}
