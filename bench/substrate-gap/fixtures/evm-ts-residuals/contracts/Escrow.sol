// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

contract Escrow {
    event Released(uint256 indexed id);

    function release(uint id) external {
        emit Released(id);
    }

    function refund(uint id) external {}
}
