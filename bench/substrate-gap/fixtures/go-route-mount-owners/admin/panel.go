package admin

import "github.com/gin-gonic/gin"

type Panel struct {
	admin *gin.RouterGroup
}

// NewPanel stores rg.Group("/admin"): the field holds rg's mount + /admin.
func NewPanel(rg *gin.RouterGroup) *Panel {
	p := &Panel{}
	p.admin = rg.Group("/admin")
	p.admin.GET("/stats", p.stats)
	return p
}

func (p *Panel) stats(c *gin.Context) {}
