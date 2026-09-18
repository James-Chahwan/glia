defmodule MyAppWeb.Router do
  use Phoenix.Router

  scope "/", MyAppWeb do
    get "/users", UserController, :index
  end
end
