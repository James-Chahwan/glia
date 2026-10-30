local http = require("socket.http")

local function fetch_user(id)
  return http.request("http://users.internal/users/" .. id)
end

return { fetch_user = fetch_user }
