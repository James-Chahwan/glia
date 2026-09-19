defmodule MyApp.Workers.HourlyWorker do
  use Oban.Worker
  def perform(_job), do: :ok
end

defmodule MyApp.Workers.DailyWorker do
  use Oban.Worker
  def perform(_job), do: :ok
end
