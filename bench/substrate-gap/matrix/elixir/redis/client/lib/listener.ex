defmodule Listener do
  def listen(pubsub) do
    {:ok, _ref} = Redix.PubSub.subscribe(pubsub, "orders", self())
  end
end
