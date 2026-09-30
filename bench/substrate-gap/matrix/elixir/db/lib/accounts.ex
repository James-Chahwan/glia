defmodule Shop.Accounts do
  import Ecto.Query

  def list_users, do: Shop.Repo.all(Shop.User)
end
