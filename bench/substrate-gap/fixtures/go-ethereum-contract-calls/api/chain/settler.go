package chain

import (
	"context"
	"math/big"

	"github.com/ethereum/go-ethereum/accounts/abi/bind"
	"github.com/ethereum/go-ethereum/common"
	"github.com/ethereum/go-ethereum/ethclient"
	"github.com/ethereum/go-ethereum/rpc"
)

type Settler struct {
	client *ethclient.Client
	raw    *rpc.Client
	escrow *bind.BoundContract
}

func NewSettler(client *ethclient.Client, raw *rpc.Client, contract *bind.BoundContract) *Settler {
	return &Settler{client: client, raw: raw, escrow: contract}
}

func (s *Settler) Release(opts *bind.TransactOpts, orderID [32]byte) error {
	_, err := s.escrow.Transact(opts, "release", orderID)
	return err
}

func (s *Settler) Held(opts *bind.CallOpts, orderID [32]byte) (*big.Int, error) {
	var out []interface{}
	if err := s.escrow.Call(opts, &out, "deposits", orderID); err != nil {
		return nil, err
	}
	return out[0].(*big.Int), nil
}

func (s *Settler) Balance(ctx context.Context, who common.Address) (*big.Int, error) {
	return s.client.BalanceAt(ctx, who, nil)
}

func (s *Settler) Head(ctx context.Context) (string, error) {
	var head string
	if err := s.raw.Call(&head, "eth_blockNumber"); err != nil {
		return "", err
	}
	return head, s.raw.CallContext(ctx, &head, "eth_chainId")
}
