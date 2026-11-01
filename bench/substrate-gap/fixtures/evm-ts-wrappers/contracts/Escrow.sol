// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

contract Escrow {
    address public arbiter;

    event Released(address indexed payee, uint256 amount);

    constructor(address a) {
        arbiter = a;
    }

    function deposit(address payee) external payable {}

    function release(address payee, uint256 amount) external {
        emit Released(payee, amount);
    }
}
