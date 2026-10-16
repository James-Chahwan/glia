package main

import (
	"github.com/99designs/gqlgen/graphql/handler"
	"github.com/gin-gonic/gin"

	"example.com/stock/api/graph"
)

func main() {
	srv := handler.NewDefaultServer(graph.NewExecutableSchema(graph.Config{Resolvers: &graph.Resolver{}}))
	r := gin.Default()
	r.POST("/graphql", gin.WrapH(srv))
	_ = r.Run(":8080")
}
