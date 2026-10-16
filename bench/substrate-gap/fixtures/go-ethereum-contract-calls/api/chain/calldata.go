package chain

import (
	"strings"

	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/accounts/abi/bind"
	"github.com/ethereum/go-ethereum/common"
)

const escrowCallsABI = `[
  {"type":"function","name":"deposit","stateMutability":"payable","inputs":[{"name":"orderId","type":"bytes32"}],"outputs":[]},
  {"type":"function","name":"release","stateMutability":"nonpayable","inputs":[{"name":"orderId","type":"bytes32"}],"outputs":[]}
]`

func DepositCalldata(id [32]byte) ([]byte, error) {
	parsed, err := abi.JSON(strings.NewReader(escrowCallsABI))
	if err != nil {
		return nil, err
	}
	return parsed.Pack("deposit", id)
}

func ReleaseVia(backend bind.ContractBackend, addr common.Address, opts *bind.TransactOpts, id [32]byte) error {
	parsed, err := abi.JSON(strings.NewReader(escrowCallsABI))
	if err != nil {
		return err
	}
	bc := bind.NewBoundContract(addr, parsed, backend, backend, backend)
	if _, err := bc.Transact(opts, "release", id); err != nil {
		return err
	}
	_, err = bc.Transact(opts, "withdraw", id)
	return err
}
