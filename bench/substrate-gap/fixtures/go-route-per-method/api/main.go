package main

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

func listUsers(c *gin.Context)  {}
func createUser(c *gin.Context) {}
func getUser(c *gin.Context)    {}
func deleteUser(c *gin.Context) {}
func health(w http.ResponseWriter, r *http.Request) {}

func main() {
	r := gin.Default()
	r.GET("/users", listUsers)
	r.POST("/users", createUser)
	r.GET("/users/:id", getUser)
	r.DELETE("/users/:id", deleteUser)
	http.HandleFunc("/health", health)
	r.Run()
}
