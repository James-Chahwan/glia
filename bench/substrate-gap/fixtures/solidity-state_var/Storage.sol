// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

contract Storage {
    uint256 public feeBasisPoints;
    address public treasury;
    mapping(address => uint256) private balances;

    function setFee(uint256 fee) public {
        feeBasisPoints = fee;
    }
}
