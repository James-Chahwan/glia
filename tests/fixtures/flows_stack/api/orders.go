package main

import (
	"database/sql"

	"github.com/gin-gonic/gin"
)

var db *sql.DB

func ListOrders(c *gin.Context) {
	rows, _ := db.Query("SELECT id FROM orders")
	defer rows.Close()
	c.JSON(200, rows)
}

func main() {
	r := gin.Default()
	r.GET("/api/orders", ListOrders)
	r.Run()
}
