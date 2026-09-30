defmodule Orders.Pipeline do
  use Broadway

  def start_link(_) do
    Broadway.start_link(__MODULE__,
      name: __MODULE__,
      producer: [module: {BroadwayKafka.Producer, hosts: [localhost: 9092], group_id: "orders", topics: ["orders"]}],
      processors: [default: []]
    )
  end

  def handle_message(_, message, _), do: message
end
