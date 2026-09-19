package main

import (
	"context"
	"net/http"

	"connectrpc.com/connect"
	elizav1 "example.com/eliza/gen/eliza/v1"
	"example.com/eliza/gen/eliza/v1/elizav1connect"
)

type elizaServer struct {
	elizav1connect.UnimplementedElizaServiceHandler
}

func (s *elizaServer) Say(ctx context.Context, req *connect.Request[elizav1.SayRequest]) (*connect.Response[elizav1.SayResponse], error) {
	return connect.NewResponse(&elizav1.SayResponse{Sentence: req.Msg.Sentence}), nil
}

func main() {
	mux := http.NewServeMux()
	path, handler := elizav1connect.NewElizaServiceHandler(&elizaServer{})
	mux.Handle(path, handler)
	http.ListenAndServe("localhost:8080", mux)
}
