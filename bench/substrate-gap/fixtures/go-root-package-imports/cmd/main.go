package main

import "example.com/rootpkg"

type conn struct{}

func (conn) Close() error { return nil }

func main() {
	c := rootpkg.NewClient("x")
	_ = rootpkg.WithTimeout(3)
	_ = c.Invoke("m")
}
