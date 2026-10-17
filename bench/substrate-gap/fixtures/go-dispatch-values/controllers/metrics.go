package controllers

import "github.com/gin-gonic/gin"

func MetricsHandler(c *gin.Context) { c.String(200, "metrics") }

// MetricsDebug is registered by nothing.
func MetricsDebug(c *gin.Context) { c.String(200, "debug") }
