defmodule Sub do
  def listen(gnat) do
    {:ok, _sid} = Gnat.sub(gnat, self(), "orders")
  end
end
