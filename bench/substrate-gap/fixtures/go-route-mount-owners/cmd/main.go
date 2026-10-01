package main

import (
	"example.com/owners/admin"
	"example.com/owners/api"
	"github.com/gin-gonic/gin"
)

func main() {
	r := gin.Default()
	// (a) a field of ANOTHER package's struct, assigned here.
	s := &api.Server{}
	s.Public = r.Group("/public")
	s.Routes()
	// (b) a group handed to a constructor that stores rg.Group(..) on a field.
	admin.NewPanel(r.Group("/api"))
}
