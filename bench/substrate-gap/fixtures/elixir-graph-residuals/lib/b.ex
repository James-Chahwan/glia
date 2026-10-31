defmodule MyApp.Web do
  alias MyApp.Accounts, as: Acc

  def show(id), do: Acc.get(id)

  def render(error), do: MyApp.Exception.status(error)
end
