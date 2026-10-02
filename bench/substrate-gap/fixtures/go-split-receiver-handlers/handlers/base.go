package handlers

import "github.com/gin-gonic/gin"

// BaseHandler is embedded by every handler: Health is promoted.
type BaseHandler struct{}

func (b *BaseHandler) Health(c *gin.Context) {}
