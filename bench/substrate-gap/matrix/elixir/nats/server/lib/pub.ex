defmodule Pub do
  def publish(gnat, body) do
    Gnat.pub(gnat, "orders", body)
  end
end
