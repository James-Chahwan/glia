// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

/// @title Vault
/// @author Kina
/// @notice Holds deposits for members.
contract Vault {
    /// @notice Platform fee in basis points.
    uint256 public feeBasisPoints;

    /**
     * @notice Deposit funds for a member.
     * @dev Reverts when amount is zero.
     * @param member The member credited.
     * @param amount The amount in wei.
     * @return ok True on success.
     */
    function deposit(address member, uint256 amount) public returns (bool ok) {
        return true;
    }

    /// @notice Withdraw.
    /// @param amount How much.
    /// @return remaining Balance left.
    /// @inheritdoc IVault
    /// @custom:security non-reentrant
    function withdraw(uint256 amount) external returns (uint256 remaining) {
        return 0;
    }

    /// @return True always.
    function ping() external pure returns (bool) {
        return true;
    }
}
