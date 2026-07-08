object Calc {
  def helper(x: Int): Int = x + 1

  def compute(n: Int): Int = {
    helper(n) * 2
  }
}
