package main

import (
	"context"
	"fmt"
	"net/http"

	pb "example.com/twirp/rpc/haberdasher"
)

func main() {
	client := pb.NewHaberdasherProtobufClient("http://localhost:8080", &http.Client{})
	hat, err := client.MakeHat(context.Background(), &pb.Size{Inches: 12})
	if err != nil {
		fmt.Printf("oh no: %v", err)
		return
	}
	fmt.Printf("I have a nice new hat: %+v", hat)
}
