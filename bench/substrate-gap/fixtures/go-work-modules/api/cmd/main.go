package main

import (
	"fmt"

	"example.com/acme/shared"
	"example.com/acme/shared/auth"
	"github.com/google/uuid"
)

func main() {
	if auth.Check("token") {
		fmt.Println(shared.Version(), uuid.NewString())
	}
}
