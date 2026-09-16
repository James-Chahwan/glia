defmodule ApiClient do
  @moduledoc "Outbound HTTP calls to the users and orders services."

  # HTTPoison: verb is the function name, URL is the first string argument.
  # `#{id}` interpolation -> `${…}` so it normalises like a TS template path.
  def fetch_user(id) do
    HTTPoison.get("http://users-svc/api/users/#{id}")
  end

  # Tesla's client-first form: the URL is the SECOND argument.
  def create_user(body) do
    Tesla.post(client(), "/api/users", body)
  end

  # Req: the URL arrives as a `url:` keyword, not positionally.
  def list_orders do
    Req.get!(url: "http://orders-svc/api/orders")
  end

  # Finch: verb is an atom first argument, URL is second.
  def remove_user(id) do
    Finch.build(:delete, "http://users-svc/api/users/#{id}")
  end

  defp client, do: Tesla.client([])

  # Negative control: a dotted `.get` on a non-HTTP receiver with a non-URL
  # string must NOT become an ENDPOINT.
  def cached(id), do: Cache.get("user." <> id)
end
