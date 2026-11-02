// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @notice The price feed the lending pool reads, a copy with one more member.
interface IPriceOracle {
    function decimals() external view returns (uint8);

    function latestPrice(address asset) external view returns (uint256);
}
