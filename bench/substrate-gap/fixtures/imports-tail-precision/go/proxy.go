package ws

import "golang.org/x/net/proxy"

func proxyDialer() proxy.Dialer {
	return proxy.Direct
}
