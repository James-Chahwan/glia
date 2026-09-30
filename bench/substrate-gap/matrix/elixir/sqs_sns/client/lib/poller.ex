defmodule Poller do
  def poll do
    ExAws.SQS.receive_message("orders") |> ExAws.request()
  end
end
