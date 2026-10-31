package wsdemo

import "net/http"

// Upgrader upgrades an HTTP connection.
type Upgrader struct {
	ReadBufferSize int
}

// Upgrade returns a connection for the request.
func (u *Upgrader) Upgrade(w http.ResponseWriter, r *http.Request) error {
	return nil
}

// Subprotocols returns the protocols the client asked for.
func Subprotocols(r *http.Request) []string {
	return nil
}
