package fake

// fakeServer declares its own unexported mustEmbed... method: an unexported
// method of ANOTHER package, so it never satisfies pb.GreeterServer.
type fakeServer struct{}

func (fakeServer) SayHello(name string) string { return "" }
func (fakeServer) mustEmbedUnimplementedGreeterServer() {}
