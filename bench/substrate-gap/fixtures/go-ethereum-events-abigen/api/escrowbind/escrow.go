// Code generated - DO NOT EDIT.
// This file is a generated binding and any manual changes will be lost.

package escrowbind

import (
	"math/big"

	"github.com/ethereum/go-ethereum/accounts/abi/bind"
	"github.com/ethereum/go-ethereum/core/types"
)

// EscrowMetaData contains all meta data concerning the Escrow contract.
var EscrowMetaData = &bind.MetaData{
	ABI: "[{\"inputs\":[{\"internalType\":\"bytes32\",\"name\":\"orderId\",\"type\":\"bytes32\"}],\"name\":\"deposit\",\"outputs\":[],\"stateMutability\":\"payable\",\"type\":\"function\"},{\"inputs\":[{\"internalType\":\"bytes32\",\"name\":\"orderId\",\"type\":\"bytes32\"}],\"name\":\"release\",\"outputs\":[],\"stateMutability\":\"nonpayable\",\"type\":\"function\"},{\"inputs\":[{\"internalType\":\"bytes32\",\"name\":\"\",\"type\":\"bytes32\"}],\"name\":\"deposits\",\"outputs\":[{\"internalType\":\"uint256\",\"name\":\"\",\"type\":\"uint256\"}],\"stateMutability\":\"view\",\"type\":\"function\"},{\"anonymous\":false,\"inputs\":[{\"indexed\":true,\"internalType\":\"bytes32\",\"name\":\"orderId\",\"type\":\"bytes32\"},{\"indexed\":false,\"internalType\":\"address\",\"name\":\"seller\",\"type\":\"address\"}],\"name\":\"Released\",\"type\":\"event\"}]",
}

type EscrowCaller struct {
	contract *bind.BoundContract
}

type EscrowTransactor struct {
	contract *bind.BoundContract
}

type EscrowFilterer struct {
	contract *bind.BoundContract
}

func (_Escrow *EscrowCaller) Deposits(opts *bind.CallOpts, arg0 [32]byte) (*big.Int, error) {
	var out []interface{}
	err := _Escrow.contract.Call(opts, &out, "deposits", arg0)
	if err != nil {
		return new(big.Int), err
	}
	return out[0].(*big.Int), nil
}

func (_Escrow *EscrowTransactor) Deposit(opts *bind.TransactOpts, orderId [32]byte) (*types.Transaction, error) {
	return _Escrow.contract.Transact(opts, "deposit", orderId)
}

func (_Escrow *EscrowFilterer) FilterReleased(opts *bind.FilterOpts, orderId [][32]byte) (chan types.Log, error) {
	var orderIdRule []interface{}
	for _, orderIdItem := range orderId {
		orderIdRule = append(orderIdRule, orderIdItem)
	}
	logs, sub, err := _Escrow.contract.FilterLogs(opts, "Released", orderIdRule)
	if err != nil {
		return nil, err
	}
	sub.Unsubscribe()
	return logs, nil
}
