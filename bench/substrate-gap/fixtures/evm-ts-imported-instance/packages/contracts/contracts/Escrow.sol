// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

/// @title Escrow holds a payer's deposit until the arbiter releases it.
contract Escrow {
    address public arbiter;
    mapping(address => uint256) public balances;

    event Deposited(address indexed payee, uint256 amount);
    event Released(address indexed payee, uint256 amount);

    constructor(address arbiter_) {
        arbiter = arbiter_;
    }

    function deposit(address payee) external payable {
        balances[payee] += msg.value;
        emit Deposited(payee, msg.value);
    }

    function release(address payee, uint256 amount, bytes32 memo) external {
        require(msg.sender == arbiter, "arbiter only");
        balances[payee] -= amount;
        payable(payee).transfer(amount);
        emit Released(payee, amount);
    }
}
