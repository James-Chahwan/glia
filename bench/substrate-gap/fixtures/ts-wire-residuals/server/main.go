package main

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

type User struct {
	ID    int    `json:"id"`
	Name  string `json:"name"`
	Email string `json:"email"`
}

func create(c *gin.Context) {
	var u User
	if err := c.ShouldBindJSON(&u); err != nil {
		c.JSON(http.StatusBadRequest, gin.H{"error": err.Error()})
		return
	}
	c.JSON(http.StatusCreated, u)
}

func list(c *gin.Context) {
	c.JSON(http.StatusOK, []User{})
}

func main() {
	r := gin.Default()
	r.POST("/users", create)
	r.GET("/users", list)
	r.Run()
}
