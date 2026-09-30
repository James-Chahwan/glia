package main

import "github.com/gin-gonic/gin"

func main() {
	r := gin.Default()
	r.GET("/health", health)
	RegisterAdmin(r.Group("/admin"))
	RegisterOps(r.Group("/ops"))
	back := r.Group("/back")
	RegisterAudit(back.Group("/office"))
	r.Run()
}

// RegisterAdmin serves the admin console on whatever group it is handed.
func RegisterAdmin(rg *gin.RouterGroup) {
	rg.POST("/users/:id/status", setStatus)
	rg.GET("/reports", adminReports)
	rg.GET("/health", adminHealth)
}

// RegisterOps serves a second `/reports`, under another mount.
func RegisterOps(rg *gin.RouterGroup) {
	rg.GET("/reports", opsReports)
}

// RegisterAudit is mounted two literal segments deep.
func RegisterAudit(rg *gin.RouterGroup) {
	rg.GET("/audit/log", auditLog)
}

func health(c *gin.Context)       {}
func setStatus(c *gin.Context)    {}
func adminReports(c *gin.Context) {}
func adminHealth(c *gin.Context)  {}
func opsReports(c *gin.Context)   {}
func auditLog(c *gin.Context)     {}
