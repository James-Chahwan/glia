package ws

import "golang.org/x/net/proxy"

// Conn is one connection.
type Conn struct{}

func dial() proxy.Dialer {
	return proxyDialer()
}
