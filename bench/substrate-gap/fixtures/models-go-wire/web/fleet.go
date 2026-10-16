package web

import (
	"net/http"

	"github.com/gin-gonic/gin"

	"example.com/fleet/models"
	"example.com/fleet/store"
)

type Fleet struct {
	store *store.Store
}

func (f *Fleet) Routes(r *gin.Engine) {
	r.GET("/vehicles", f.List)
	r.GET("/vehicles/:id", f.Get)
	r.POST("/vehicles", f.Register)
	r.GET("/vehicles/:id/trips", f.Trips)
}

func (f *Fleet) List(c *gin.Context) {
	c.JSON(http.StatusOK, f.store.Vehicles())
}

func (f *Fleet) Get(c *gin.Context) {
	v, ok := f.store.Vehicle(c.Param("id"))
	if !ok {
		c.JSON(http.StatusNotFound, models.Problem{Code: "not_found", Message: "no vehicle"})
		return
	}
	c.JSON(http.StatusOK, v)
}

func (f *Fleet) Register(c *gin.Context) {
	var req models.RegisterVehicle
	if err := c.ShouldBindJSON(&req); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}
	v := &models.Vehicle{ID: "v-1", Plate: req.Plate}
	c.JSON(http.StatusCreated, v)
}

func (f *Fleet) Trips(c *gin.Context) {
	trips := f.store.Trips(c.Param("id"))
	c.JSON(http.StatusOK, gin.H{"trips": trips, "count": len(trips)})
}
