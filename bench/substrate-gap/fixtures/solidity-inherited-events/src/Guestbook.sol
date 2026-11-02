// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import "./interfaces/IGuestbook.sol";

contract Guestbook is IGuestbook {
    event Opened(address owner);

    uint256 private entries;

    function open() external {
        emit Opened(msg.sender);
    }

    function sign(string calldata note) external {
        entries += 1;
        emit Signed(msg.sender, note);
    }

    function clear() external {
        uint256 count = entries;
        entries = 0;
        emit IGuestbook.Cleared(count);
    }
}
