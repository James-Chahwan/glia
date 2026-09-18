package main

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

func listUsers(c *gin.Context)   {}
func createUser(c *gin.Context)  {}
func patchUser(c *gin.Context)   {}
func anyPing(c *gin.Context)     {}
func matchOrders(c *gin.Context) {}
func getItem(w http.ResponseWriter, r *http.Request) {}

func main() {
	r := gin.Default()
	r.GET("/users", listUsers)
	r.POST("/users", createUser)
	r.Handle("PATCH", "/users/:id", patchUser)
	r.Any("/ping", anyPing)
	r.Match([]string{"GET", "POST"}, "/orders", matchOrders)
	mux := http.NewServeMux()
	mux.HandleFunc("GET /items/{id}", getItem)
	r.Run()
}
