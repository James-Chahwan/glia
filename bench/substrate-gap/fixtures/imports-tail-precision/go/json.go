package ws

import "encoding/json"

// WriteJSON encodes v onto the connection's writer.
func (c *Conn) WriteJSON(v interface{}) error {
	_, err := json.Marshal(v)
	return err
}
