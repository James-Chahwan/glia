defmodule Checkout do
  def variant(user) do
    if FunWithFlags.enabled?(:"new-checkout", for: user), do: "new", else: "legacy"
  end
end
