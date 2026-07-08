defmodule MyApp.Math do
  def compute(x) do
    helper(x) + 1
  end

  def helper(x) do
    x * 2
  end
end
