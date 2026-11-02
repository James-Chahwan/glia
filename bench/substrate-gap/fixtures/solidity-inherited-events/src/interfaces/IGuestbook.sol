// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

interface IGuestbook {
    event Signed(address indexed guest, string note);
    event Cleared(uint256 count);

    function sign(string calldata note) external;
}
