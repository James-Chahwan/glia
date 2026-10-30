package main

import (
	"context"
	"net"

	pb "github.com/example/demo/genproto/hipstershop"
	"google.golang.org/grpc"
	"google.golang.org/grpc/health"
	healthpb "google.golang.org/grpc/health/grpc_health_v1"
)

type server struct {
	pb.UnimplementedShippingServiceServer
}

func main() {
	lis, _ := net.Listen("tcp", ":50051")
	srv := grpc.NewServer()
	svc := &server{}
	pb.RegisterShippingServiceServer(srv, svc)
	healthcheck := health.NewServer()
	healthpb.RegisterHealthServer(srv, healthcheck)
	srv.Serve(lis)
}

func (s *server) Check(ctx context.Context, req *healthpb.HealthCheckRequest) (*healthpb.HealthCheckResponse, error) {
	return &healthpb.HealthCheckResponse{}, nil
}

func (s *server) GetQuote(ctx context.Context, in *pb.GetQuoteRequest) (*pb.GetQuoteResponse, error) {
	return &pb.GetQuoteResponse{}, nil
}
