defmodule Shop.UserRepo do
  @callback all() :: [map()]
end

defmodule Shop.PgUserRepo do
  @behaviour Shop.UserRepo

  def all, do: []
end
