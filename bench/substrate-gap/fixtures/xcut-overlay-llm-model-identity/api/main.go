package main

import "github.com/gin-gonic/gin"

type User struct {
	ID   int    `json:"id"`
	Name string `json:"name"`
}

type Account struct {
	ID   int    `json:"id"`
	Plan string `json:"plan"`
}

func listUsers(c *gin.Context) {
	users := []User{{ID: 1, Name: "ada"}}
	c.JSON(200, users)
}

func listAccounts(c *gin.Context) {
	accounts := []Account{{ID: 1, Plan: "pro"}}
	c.JSON(200, accounts)
}

func main() {
	r := gin.Default()
	r.GET("/api/users", listUsers)
	r.GET("/api/accounts", listAccounts)
	r.Run()
}
