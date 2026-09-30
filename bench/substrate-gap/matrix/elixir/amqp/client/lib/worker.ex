defmodule Worker do
  def start(chan) do
    AMQP.Queue.declare(chan, "orders", durable: true)
    AMQP.Basic.consume(chan, "orders")
  end
end
