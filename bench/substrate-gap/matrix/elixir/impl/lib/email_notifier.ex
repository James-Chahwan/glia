defmodule Shop.EmailNotifier do
  @behaviour Shop.Notifier

  @impl true
  def notify(_msg), do: :ok
end
