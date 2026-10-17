package server

import (
	controllers "example.com/shop/controllers"

	"github.com/gin-gonic/gin"
)

func Setup() *gin.Engine {
	r := gin.New()
	r.GET("/ping", controllers.Ping)
	r.GET("/metrics", controllers.MetricsHandler)
	r.GET("/healthz", controllers.HealthHandler)
	return r
}
