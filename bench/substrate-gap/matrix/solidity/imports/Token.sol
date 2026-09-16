// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import "./IERC20.sol";

contract Token {
    IERC20 public reserve;

    function backing() external view returns (uint256) {
        return reserve.totalSupply();
    }
}
