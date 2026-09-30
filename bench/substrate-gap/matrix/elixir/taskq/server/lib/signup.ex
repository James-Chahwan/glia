defmodule MyApp.Signup do
  def register(user_id) do
    %{user_id: user_id}
    |> MyApp.MailerWorker.new()
    |> Oban.insert()
  end
end
