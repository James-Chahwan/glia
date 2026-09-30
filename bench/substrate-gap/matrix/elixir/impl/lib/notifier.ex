defmodule Shop.Notifier do
  @callback notify(String.t()) :: :ok
end
