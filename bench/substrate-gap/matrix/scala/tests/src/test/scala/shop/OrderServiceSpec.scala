package shop

import org.scalatest.flatspec.AnyFlatSpec

class OrderServiceSpec extends AnyFlatSpec {
  "OrderService" should "sum" in {
    assert(new OrderService().total(List(1, 2)) == 3)
  }
}
