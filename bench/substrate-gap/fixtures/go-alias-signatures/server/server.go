package server

import "example.com/aliases/pb"

// ChatService names the generated stream alias: the same type as the
// interface's grpc.BidiStreamingServer[Msg, Reply].
type ChatService struct{}

func (s *ChatService) Stream(stream pb.Chat_StreamServer) error { return nil }

func (s *ChatService) Send(m *pb.Msg) (*pb.Reply, error) { return nil, nil }

// FeedService: the quokka chat shape, an embedded Unimplemented server plus
// an override typed by the alias.
type FeedService struct {
	pb.UnimplementedFeedServer
}

func (f *FeedService) Watch(m *pb.Msg, stream pb.Feed_WatchServer) error { return nil }

// WrongChat's Stream takes the Feed alias: a different instantiation.
type WrongChat struct{}

func (w *WrongChat) Stream(stream pb.Feed_WatchServer) error { return nil }

func (w *WrongChat) Send(m *pb.Msg) (*pb.Reply, error) { return nil, nil }
