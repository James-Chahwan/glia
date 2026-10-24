defmodule MyApp.Rooms do
  # Phoenix.PubSub.broadcast(MyApp.PubSub, "ghost_ex", msg)
  @doc "Phoenix.PubSub.broadcast(MyApp.PubSub, \"ghost_ex\", msg)"
  def notify(msg) do
    Phoenix.PubSub.broadcast(MyApp.PubSub, "rooms", msg)
  end
end
