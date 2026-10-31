defmodule MyApp.AccountsPropertyTest do
  use ExUnit.Case
  use ExUnitProperties

  property "get returns its id" do
    check all id <- integer() do
      assert MyApp.Accounts.get(id) == id
    end
  end
end
