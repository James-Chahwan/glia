package main

import "google.golang.org/grpc"

type geocoderServer struct{ UnimplementedGeocoderServer }

func main() {
	s := grpc.NewServer()
	RegisterGeocoderServer(s, &geocoderServer{})
}
