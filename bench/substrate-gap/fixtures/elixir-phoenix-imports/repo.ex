defmodule MyApp.Repo do
  def all(queryable), do: queryable

  def get(queryable, _id), do: queryable
end
