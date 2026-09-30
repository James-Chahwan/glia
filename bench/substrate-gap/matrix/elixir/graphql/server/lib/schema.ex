defmodule ShopWeb.Schema do
  use Absinthe.Schema

  object :order do
    field :id, :id
  end

  query do
    field :orders, list_of(:order) do
      resolve fn _, _, _ -> {:ok, []} end
    end
  end
end
