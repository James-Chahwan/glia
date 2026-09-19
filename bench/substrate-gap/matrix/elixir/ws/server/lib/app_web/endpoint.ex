defmodule AppWeb.Endpoint do
  use Phoenix.Endpoint, otp_app: :app

  socket "/socket", AppWeb.UserSocket, websocket: true, longpoll: false
end
