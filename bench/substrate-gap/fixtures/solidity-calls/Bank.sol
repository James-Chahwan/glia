// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

contract Bank {
    mapping(address => uint256) private balances;

    function deposit(uint256 amount) public {
        _credit(msg.sender, amount);
    }

    function _credit(address who, uint256 amount) internal {
        balances[who] += amount;
    }
}
