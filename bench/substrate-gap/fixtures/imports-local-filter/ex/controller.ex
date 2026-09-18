defmodule MyAppWeb.UserController do
  alias MyApp.Repo
  import Plug.Conn

  def index(conn), do: Repo.all()
end
