package router

import "github.com/gin-gonic/gin"

// LogRouter keeps its chained group on a struct field.
type LogRouter struct {
	private gin.IRoutes
}

func getLogList(c *gin.Context) {}

// NewLogRouter stores the chained group on the field.
func NewLogRouter(Router *gin.RouterGroup) *LogRouter {
	s := &LogRouter{}
	s.private = Router.Group("sysLog").Use(OperationRecord())
	return s
}

// Routes registers on the field another method assigned.
func (s *LogRouter) Routes() {
	s.private.GET("getLogList", getLogList)
}
