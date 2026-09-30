defmodule Listener do
  use Broadway

  def start_link(_) do
    Broadway.start_link(__MODULE__,
      name: __MODULE__,
      producer: [module: {BroadwayCloudPubSub.Producer, subscription: "projects/shop/subscriptions/orders"}],
      processors: [default: []]
    )
  end

  def handle_message(_, message, _), do: message
end
