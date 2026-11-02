// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @notice The price feed the vault reads.
interface IPriceOracle {
    function latestPrice(address asset) external view returns (uint256);
}
