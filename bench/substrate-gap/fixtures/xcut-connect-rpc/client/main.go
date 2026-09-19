package main

import (
	"context"
	"log"
	"net/http"

	"connectrpc.com/connect"
	elizav1 "example.com/eliza/gen/eliza/v1"
	"example.com/eliza/gen/eliza/v1/elizav1connect"
)

func main() {
	client := elizav1connect.NewElizaServiceClient(http.DefaultClient, "http://localhost:8080")
	res, err := client.Say(context.Background(), connect.NewRequest(&elizav1.SayRequest{Sentence: "hi"}))
	if err != nil {
		log.Fatal(err)
	}
	log.Println(res.Msg.Sentence)
}
