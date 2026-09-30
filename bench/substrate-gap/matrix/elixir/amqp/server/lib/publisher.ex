defmodule Publisher do
  def publish(chan, body) do
    AMQP.Basic.publish(chan, "", "orders", body)
  end
end
