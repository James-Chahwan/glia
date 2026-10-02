package pb

type GreeterServer interface {
	SayHello(name string) string
	mustEmbedUnimplementedGreeterServer()
}

type UnimplementedGreeterServer struct{}

func (UnimplementedGreeterServer) SayHello(name string) string { return "" }
func (UnimplementedGreeterServer) mustEmbedUnimplementedGreeterServer() {}
