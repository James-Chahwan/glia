// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

contract Escrow {
    mapping(bytes32 => uint256) public deposits;

    event Released(bytes32 indexed orderId, address seller);

    function deposit(bytes32 orderId) external payable {
        deposits[orderId] += msg.value;
    }

    function release(bytes32 orderId) external {
        deposits[orderId] = 0;
        emit Released(orderId, msg.sender);
    }
}
