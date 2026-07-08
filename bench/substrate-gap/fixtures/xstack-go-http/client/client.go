package client

import (
	"io"
	"net/http"
)

func FetchUsers() ([]byte, error) {
	resp, err := http.Get("http://api.example.com/users")
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	return io.ReadAll(resp.Body)
}
