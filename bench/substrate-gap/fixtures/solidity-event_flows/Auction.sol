// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

contract Auction {
    uint256 public highestBid;

    event BidPlaced(address indexed bidder, uint256 amount);

    function bid(uint256 amount) public {
        highestBid = amount;
        emit BidPlaced(msg.sender, amount);
    }
}
