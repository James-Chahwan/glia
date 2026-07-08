defmodule MyAppWeb.UserController do
  use MyAppWeb, :controller
  import Plug.Conn
  alias MyApp.Repo

  def index(conn, _params) do
    users = Repo.all(User)
    json(conn, users)
  end
end
