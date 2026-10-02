package pb

import "google.golang.org/grpc"

type Msg struct {
	Text string
}

type Reply struct {
	Text string
}

// ChatServer: generated without mustEmbed (require_unimplemented_servers=false).
type ChatServer interface {
	Stream(grpc.BidiStreamingServer[Msg, Reply]) error
	Send(*Msg) (*Reply, error)
}

// This type alias is provided for backwards compatibility with existing code
// that references the prior non-generic stream type by name.
type Chat_StreamServer = grpc.BidiStreamingServer[Msg, Reply]

type FeedServer interface {
	Watch(*Msg, grpc.ServerStreamingServer[Reply]) error
	mustEmbedUnimplementedFeedServer()
}

type UnimplementedFeedServer struct{}

func (UnimplementedFeedServer) Watch(*Msg, grpc.ServerStreamingServer[Reply]) error { return nil }
func (UnimplementedFeedServer) mustEmbedUnimplementedFeedServer()                   {}

type Feed_WatchServer = grpc.ServerStreamingServer[Reply]
