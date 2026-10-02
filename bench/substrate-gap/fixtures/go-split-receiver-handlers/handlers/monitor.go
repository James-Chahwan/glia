package handlers

import "github.com/gin-gonic/gin"

// Monitor also has a Health: the repo-unique method fallback cannot pick.
type Monitor struct{}

func (m Monitor) Health(c *gin.Context) {}
