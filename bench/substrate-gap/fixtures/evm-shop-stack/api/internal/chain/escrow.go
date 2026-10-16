package chain

import (
	"context"
	"math/big"

	"github.com/ethereum/go-ethereum/accounts/abi/bind"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/ethclient"
)

type Settler struct {
	rpc    *ethclient.Client
	escrow *bind.BoundContract
	payee  common.Address
}

func Connect(ctx context.Context, url string, payee common.Address, contract *bind.BoundContract) (*Settler, error) {
	rpc, err := ethclient.DialContext(ctx, url)
	if err != nil {
		return nil, err
	}
	return &Settler{rpc: rpc, escrow: contract, payee: payee}, nil
}

func (s *Settler) Release(opts *bind.TransactOpts, orderID [32]byte) error {
	_, err := s.escrow.Transact(opts, "release", orderID)
	return err
}

func (s *Settler) Payout(ctx context.Context) (*big.Int, error) {
	return s.rpc.BalanceAt(ctx, s.payee, nil)
}
