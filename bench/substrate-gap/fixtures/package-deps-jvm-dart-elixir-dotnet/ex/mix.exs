defmodule Ex.MixProject do
  use Mix.Project

  def project do
    [app: :ex, version: "0.1.0", deps: deps()]
  end

  defp deps, do: [{:jason, "~> 1.4"}]
end
