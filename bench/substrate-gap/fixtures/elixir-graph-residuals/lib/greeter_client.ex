defmodule MyApp.GreeterClient do
  @spec hello(GRPC.Channel.t(), map()) :: term()
  def hello(channel, req), do: Helloworld.Greeter.Stub.say_hello(channel, req)
end
