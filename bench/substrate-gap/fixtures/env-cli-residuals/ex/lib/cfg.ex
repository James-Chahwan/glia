defmodule Shop.Cfg do
  def api_key, do: Application.get_env(:shop, :api_key)
end
