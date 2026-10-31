package router

import "github.com/gin-gonic/gin"

// OperationRecord is a middleware: gin's RouterGroup.Use returns the same
// group, so the chained group keeps its prefix.
func OperationRecord() gin.HandlerFunc {
	return func(c *gin.Context) { c.Next() }
}

// CasbinHandler is a second middleware.
func CasbinHandler() gin.HandlerFunc {
	return func(c *gin.Context) { c.Next() }
}

func createAuthority(c *gin.Context)  {}
func getAuthorityList(c *gin.Context) {}
func addMenu(c *gin.Context)          {}

// InitAuthorityRouter registers one prefix through a chained group (with a
// middleware) and a plain group.
func InitAuthorityRouter(Router *gin.RouterGroup) {
	authorityRouter := Router.Group("authority").Use(OperationRecord())
	authorityRouterWithoutRecord := Router.Group("authority")
	{
		authorityRouter.POST("createAuthority", createAuthority)
	}
	{
		authorityRouterWithoutRecord.POST("getAuthorityList", getAuthorityList)
	}
}

// InitMenuRouter chains two middlewares onto its group.
func InitMenuRouter(Router *gin.RouterGroup) {
	menuRouter := Router.Group("menu").Use(OperationRecord()).Use(CasbinHandler())
	menuRouter.POST("addMenu", addMenu)
}
