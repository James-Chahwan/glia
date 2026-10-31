package main

import (
	"example.com/admin/router"
	"github.com/gin-gonic/gin"
)

func main() {
	r := gin.Default()
	private := r.Group("")
	router.InitAuthorityRouter(private)
	router.InitMenuRouter(private)
	router.NewLogRouter(private).Routes()
	_ = r.Run()
}
