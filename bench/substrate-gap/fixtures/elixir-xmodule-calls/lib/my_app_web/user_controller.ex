defmodule MyAppWeb.UserController do
  alias MyApp.Accounts

  def show(conn, id) do
    Accounts.get_user(id)
  end
end
