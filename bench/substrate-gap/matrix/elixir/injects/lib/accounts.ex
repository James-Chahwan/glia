defmodule Shop.Accounts do
  @repo Application.compile_env(:shop, :user_repo, Shop.PgUserRepo)

  def list_users, do: @repo.all()
end
