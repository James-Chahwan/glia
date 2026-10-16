// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

interface IEscrow {
    function deposit(bytes32 orderId) external payable;
}

library Fees {
    function cut(uint256 amount) external pure returns (uint256) {
        return amount / 100;
    }
}

contract Escrow is IEscrow {
    address public immutable seller;
    mapping(bytes32 => uint256) public deposits;
    uint256 private total;

    event Deposited(bytes32 indexed orderId, uint256 amount);

    constructor(address seller_) {
        seller = seller_;
    }

    function deposit(bytes32 orderId) external payable {
        _credit(orderId, msg.value);
        emit Deposited(orderId, msg.value);
    }

    function release(bytes32 orderId) public {
        _credit(orderId, 0);
    }

    function release(bytes32 orderId, address to) public {
        require(to != address(0), "to");
        _credit(orderId, 0);
    }

    function _credit(bytes32 orderId, uint256 amount) internal {
        deposits[orderId] += amount;
        total += amount;
    }

    receive() external payable {}
}
