package server

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

// CreateActivity implements POST /api/protected/activity.
func CreateActivity(c *gin.Context) {
	c.JSON(http.StatusCreated, gin.H{"id": "a1"})
}

// GetActivity implements GET /api/protected/activity/:id.
func GetActivity(c *gin.Context) {
	c.JSON(http.StatusOK, gin.H{"id": c.Param("id")})
}

// Health is the liveness probe; no feature declares it.
func Health(c *gin.Context) {
	c.String(http.StatusOK, "ok")
}

// Register wires the routes. POST /api/protected/activity/:id/leave is
// declared in features/activities/feature.yaml but not implemented yet.
func Register(r *gin.Engine) {
	r.POST("/api/protected/activity", CreateActivity)
	r.GET("/api/protected/activity/:id", GetActivity)
	r.GET("/healthz", Health)
}
