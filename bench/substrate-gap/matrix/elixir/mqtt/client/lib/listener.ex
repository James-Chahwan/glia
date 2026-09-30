defmodule Listener do
  def start do
    Tortoise311.Supervisor.start_child(
      client_id: "listener",
      handler: {Tortoise311.Handler.Logger, []},
      server: {Tortoise311.Transport.Tcp, host: "broker", port: 1883},
      subscriptions: [{"sensors/temp", 0}]
    )
  end
end
