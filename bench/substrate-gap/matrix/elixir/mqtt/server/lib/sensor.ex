defmodule Sensor do
  def report do
    Tortoise311.publish("sensor", "sensors/temp", "21", qos: 0)
  end
end
