package chain

import (
	"strings"

	"github.com/ethereum/go-ethereum/accounts/abi"
)

func ReleaseCalldata(id [32]byte) ([]byte, error) {
	parsed, err := abi.JSON(strings.NewReader(shopABI))
	if err != nil {
		return nil, err
	}
	return parsed.Pack("release", id)
}
