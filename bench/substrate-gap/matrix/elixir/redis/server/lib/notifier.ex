defmodule Notifier do
  def notify(conn, body) do
    Redix.command(conn, ["PUBLISH", "orders", body])
  end
end
