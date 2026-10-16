// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

contract Pauser {
    event TradeFunded(uint256 indexed id, uint256 at);

    function confirmPayment(uint256 id) external {
        emit TradeFunded(id, block.timestamp);
    }
}
