package indexer

import (
	"strings"

	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/core/types"
)

const escrowABI = `[
  {"anonymous":false,"inputs":[{"indexed":true,"name":"orderId","type":"bytes32"},{"name":"amount","type":"uint256"}],"name":"Deposited","type":"event"},
  {"anonymous":false,"inputs":[{"indexed":true,"name":"orderId","type":"bytes32"},{"name":"seller","type":"address"}],"name":"Released","type":"event"}
]`

type Indexer struct {
	parsedABI abi.ABI
}

func New() (*Indexer, error) {
	parsed, err := abi.JSON(strings.NewReader(escrowABI))
	if err != nil {
		return nil, err
	}
	return &Indexer{parsedABI: parsed}, nil
}

func (idx *Indexer) Handle(vlog types.Log) string {
	for name, event := range idx.parsedABI.Events {
		if event.ID != vlog.Topics[0] {
			continue
		}
		switch name {
		case "Deposited":
			return "deposited"
		case "Released":
			return "released"
		}
	}
	return ""
}
