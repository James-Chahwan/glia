defmodule MyApp.MailerWorker do
  use Oban.Worker, queue: :mailers

  @impl Oban.Worker
  def perform(%Oban.Job{args: args}) do
    IO.inspect(args)
    :ok
  end
end
