defmodule CartTest do
  use ExUnit.Case

  test "sums" do
    assert Cart.total([1, 2]) == 3
  end
end
