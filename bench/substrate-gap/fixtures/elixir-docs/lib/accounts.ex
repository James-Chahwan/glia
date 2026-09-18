defmodule MyApp.Accounts do
  @moduledoc """
  Account management context.
  """

  @doc """
  Fetches a user by id.
  """
  @spec get_user(integer) :: map
  def get_user(id), do: %{id: id}

  @doc "Creates a user."
  def create_user(attrs), do: attrs

  # Hidden helper comment.
  @doc false
  def internal(x), do: x

  # Plain comment above.
  def comment_only(x), do: x
end
