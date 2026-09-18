defmodule MyAppWeb.UserController do
  use MyAppWeb, :controller

  def index(conn, _params) do
    json(conn, [])
  end
end
