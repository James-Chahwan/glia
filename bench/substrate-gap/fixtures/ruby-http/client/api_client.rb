require 'net/http'
require 'uri'
require 'faraday'

# Outbound HTTP client. Deliberately NOT named routes.rb (so the Rails routes
# scanner cannot fire) and it contains no bare top-level `get '/x' do` block
# (so the Sinatra scanner cannot fire either) — any ROUTE node here is a phantom.
class ApiClient
  # Net::HTTP.get(URI(...)) -> GET /api/users/${…} (host stripped, #{} -> ${…}).
  def fetch_user(id)
    Net::HTTP.get(URI("http://users-svc/api/users/#{id}"))
  end

  # Faraday connection, literal relative path -> POST /api/users.
  def create_user(body)
    conn = Faraday.new
    conn.post('/api/users', body)
  end

  # NEGATIVE CONTROL: `.get` on a cache with a non-URL string key — the case
  # only `url_to_path` can reject.
  def cached(id)
    @cache.get("user:#{id}")
  end
end
