@testable import App
import XCTest

final class CartTests: XCTestCase {
    func testCheckout() {
        let cart = Cart(repo: Repo())
        XCTAssertEqual(cart.checkout(), 2)
    }
}
