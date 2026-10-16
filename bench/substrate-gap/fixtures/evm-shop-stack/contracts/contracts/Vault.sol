// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

contract Vault {
    event Released(bytes32 indexed id, address to);

    function release(bytes32 id) external {
        emit Released(id, msg.sender);
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        return spender != address(0) && amount > 0;
    }
}
