package server

import "example.com/impl/pb"

type server struct {
	pb.UnimplementedGreeterServer
}

func (s *server) SayHello(name string) string { return name }
