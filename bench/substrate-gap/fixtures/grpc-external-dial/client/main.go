package main

import (
	"context"

	"google.golang.org/grpc"
)

func Lookup() {
	conn, _ := grpc.Dial("geocoder.googleapis.com:443")
	c := NewGeocoderClient(conn)
	_, _ = c.Lookup(context.Background(), nil)
}
