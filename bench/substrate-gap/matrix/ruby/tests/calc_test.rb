require 'minitest/autorun'
require_relative 'calc'

class CalcTest < Minitest::Test
  def test_adds
    assert_equal 5, Calc.new.add(2, 3)
  end
end
