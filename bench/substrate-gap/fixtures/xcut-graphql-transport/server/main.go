package main

import (
	"log"

	"github.com/99designs/gqlgen/graphql/handler"
	"github.com/gin-gonic/gin"
)

func main() {
	srv := handler.NewDefaultServer(nil)
	r := gin.Default()
	r.POST("/graphql", gin.WrapH(srv))
	r.GET("/graphql/playground", gin.WrapH(srv))
	log.Fatal(r.Run(":8080"))
}
