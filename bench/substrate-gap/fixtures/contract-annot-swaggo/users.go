package main

import "github.com/gin-gonic/gin"

// GetUser godoc
// @Summary      Get a user
// @ID           getUser
// @Produce      json
// @Success      200  {object}  User
// @Failure      404  {object}  ErrorBody
// @Router       /users/{id} [get]
func GetUser(c *gin.Context) {
	c.JSON(200, User{ID: c.Param("id")})
}

// Health is a plain handler with an ordinary comment.
func Health(c *gin.Context) {
	c.JSON(200, gin.H{"ok": true})
}

type User struct {
	ID string `json:"id"`
}

type ErrorBody struct {
	Message string `json:"message"`
}

func main() {
	r := gin.Default()
	r.GET("/users/:id", GetUser)
	r.GET("/health", Health)
	r.Run()
}
