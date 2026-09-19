package main

import (
	"context"
	"net/http"

	pb "example.com/twirp/rpc/haberdasher"
)

type HaberdasherServer struct{}

func (s *HaberdasherServer) MakeHat(ctx context.Context, size *pb.Size) (*pb.Hat, error) {
	return &pb.Hat{Inches: size.Inches, Color: "blue", Name: "bowler"}, nil
}

func main() {
	twirpHandler := pb.NewHaberdasherServer(&HaberdasherServer{})
	http.ListenAndServe(":8080", twirpHandler)
}
