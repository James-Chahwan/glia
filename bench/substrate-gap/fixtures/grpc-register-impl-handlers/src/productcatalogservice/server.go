package main

import (
	"net"

	pb "github.com/example/demo/genproto/hipstershop"
	"google.golang.org/grpc"
	"google.golang.org/grpc/health"
	healthpb "google.golang.org/grpc/health/grpc_health_v1"
)

func main() {
	run("3550")
}

func run(port string) string {
	l, err := net.Listen("tcp", ":"+port)
	if err != nil {
		return ""
	}
	srv := grpc.NewServer()
	svc := &productCatalog{}
	pb.RegisterProductCatalogServiceServer(srv, svc)
	healthpb.RegisterHealthServer(srv, health.NewServer())
	go srv.Serve(l)
	return l.Addr().String()
}
