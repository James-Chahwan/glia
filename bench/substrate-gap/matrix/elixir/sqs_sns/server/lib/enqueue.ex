defmodule Enqueue do
  def enqueue(body) do
    ExAws.SQS.send_message("orders", body) |> ExAws.request()
  end
end
