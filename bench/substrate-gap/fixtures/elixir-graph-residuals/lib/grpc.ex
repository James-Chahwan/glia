defmodule MyApp.Greeter.Server do
  use GRPC.Server, service: Helloworld.Greeter.Service

  def say_hello(_r, _s), do: nil
end
