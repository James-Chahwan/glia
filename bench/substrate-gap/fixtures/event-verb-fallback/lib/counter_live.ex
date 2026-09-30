defmodule ShopWeb.CounterLive do
  use Phoenix.LiveView

  def handle_event("inc", _params, socket) do
    {:noreply, update(socket, :count, &(&1 + 1))}
  end
end
