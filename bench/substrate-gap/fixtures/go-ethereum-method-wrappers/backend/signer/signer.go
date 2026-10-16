package signer

import (
	"context"
	"log"
	"math/big"
	"strings"

	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/common"
)

const escrowManagerABI = `[
  {"inputs":[{"name":"tradeId","type":"uint256"},{"name":"winner","type":"address"},{"name":"method","type":"uint8"}],"name":"resolveDispute","outputs":[],"stateMutability":"nonpayable","type":"function"},
  {"inputs":[{"name":"tradeId","type":"uint256"}],"name":"freeze","outputs":[],"stateMutability":"nonpayable","type":"function"},
  {"inputs":[{"name":"tradeId","type":"uint256"}],"name":"unfreeze","outputs":[],"stateMutability":"nonpayable","type":"function"}
]`

type Signer struct{}

func (s *Signer) submitTo(ctx context.Context, chainName, contractAddr, method string, args ...any) ([]byte, error) {
	parsedABI, err := abi.JSON(strings.NewReader(escrowManagerABI))
	if err != nil {
		return nil, err
	}
	return parsedABI.Pack(method, args...)
}

func (s *Signer) submit(ctx context.Context, chainName, method string, args ...any) ([]byte, error) {
	return s.submitTo(ctx, chainName, "", method, args...)
}

func (s *Signer) logCall(method string) {
	log.Printf("calling %s", method)
}

func (s *Signer) ResolveDispute(ctx context.Context, chain string, id *big.Int, winner string, method uint8) ([]byte, error) {
	s.logCall("resolveDispute")
	return s.submit(ctx, chain, "resolveDispute", id, common.HexToAddress(winner), method)
}

func (s *Signer) Freeze(ctx context.Context, chain string, id *big.Int) ([]byte, error) {
	return s.submit(ctx, chain, "freeze", id)
}

func (s *Signer) Unfreeze(ctx context.Context, chain string, id *big.Int) ([]byte, error) {
	return s.submit(ctx, chain, "unfreeze", id)
}

func (s *Signer) Forward(ctx context.Context, chain, name string, id *big.Int) ([]byte, error) {
	return s.submit(ctx, chain, name, id)
}

func (s *Signer) Retry(ctx context.Context, chain string, id *big.Int) ([]byte, error) {
	m := "freeze"
	return s.submit(ctx, chain, m, id)
}
