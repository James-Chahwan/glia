// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

contract EscrowManager {
    function createTrade(address buyer, uint256 amount) external returns (uint256) {
        return amount;
    }

    function resolveDispute(uint256 tradeId, address winner, uint8 method) external {}

    function freeze(uint256 tradeId) external {}

    function unfreeze(uint256 tradeId) external {}
}
