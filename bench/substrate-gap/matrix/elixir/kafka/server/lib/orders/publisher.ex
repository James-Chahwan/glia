defmodule Orders.Publisher do
  def publish(body) do
    :brod.produce_sync(:kafka_client, "orders", :hash, "key", body)
  end
end
