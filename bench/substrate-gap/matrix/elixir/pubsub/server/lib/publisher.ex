defmodule Publisher do
  alias GoogleApi.PubSub.V1.Api.Projects

  def publish(conn, body) do
    Projects.pubsub_projects_topics_publish(conn, "shop", "orders", body: %{messages: [%{data: Base.encode64(body)}]})
  end
end
