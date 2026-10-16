package signer

import (
	"context"
	"strings"

	"github.com/ethereum/go-ethereum/accounts/abi"
)

const shopEscrowABI = `[
  {"type":"function","name":"confirm","inputs":[{"name":"orderId","type":"bytes32"}],"outputs":[],"stateMutability":"nonpayable"},
  {"type":"function","name":"release","inputs":[{"name":"orderId","type":"bytes32"}],"outputs":[],"stateMutability":"nonpayable"}
]`

type Signer struct{}

func (s *Signer) submitTo(ctx context.Context, chainName, contractAddr, method string, args ...any) ([]byte, error) {
	parsedABI, err := abi.JSON(strings.NewReader(shopEscrowABI))
	if err != nil {
		return nil, err
	}
	return parsedABI.Pack(method, args...)
}

func (s *Signer) submit(ctx context.Context, chainName, method string, args ...any) ([]byte, error) {
	return s.submitTo(ctx, chainName, "", method, args...)
}

func (s *Signer) Confirm(ctx context.Context, chain string, id [32]byte) ([]byte, error) {
	return s.submit(ctx, chain, "confirm", id)
}
