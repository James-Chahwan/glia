defmodule Orders do
  def place(order) do
    Phoenix.PubSub.broadcast(Shop.PubSub, "order_placed", {:order_placed, order})
  end

  def listen do
    Phoenix.PubSub.subscribe(Shop.PubSub, "order_placed")
  end
end
