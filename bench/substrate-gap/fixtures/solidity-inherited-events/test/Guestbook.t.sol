// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import "../src/Guestbook.sol";
import "../src/interfaces/IGuestbook.sol";

contract GuestbookTest {
    Guestbook internal book;

    function testSignEmits() external {
        book = new Guestbook();
        emit IGuestbook.Signed(address(this), "hello");
        book.sign("hello");
    }
}
