package api

import (
	"github.com/gin-gonic/gin"

	"example.com/shop/paths"
)

const apiPrefix = "/api/v1"

const (
	AdminPrefix = "/admin"
)

func Register(r *gin.Engine) {
	v1 := r.Group(apiPrefix)
	v1.GET("/items", listItems)
	admin := r.Group(AdminPrefix + "/x")
	admin.GET("/stats", stats)
	admin.GET("/users", users)
	other := r.Group(OtherFilePrefix)
	other.GET("/thing", thing)
	shared := r.Group(paths.Shared)
	shared.GET("/ping", ping)
}

func Mount(g *gin.RouterGroup) {
	sub := g.Group(apiPrefix)
	sub.GET("/deep", deep)
}

func Wire(r *gin.Engine) {
	Mount(r.Group("/m"))
}

func listItems(c *gin.Context) {}
func stats(c *gin.Context)     {}
func thing(c *gin.Context)     {}
func ping(c *gin.Context)      {}
func deep(c *gin.Context)      {}
