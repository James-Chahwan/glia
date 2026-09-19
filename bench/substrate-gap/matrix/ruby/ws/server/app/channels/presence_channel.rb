class PresenceChannel < ApplicationCable::Channel
  def subscribed
    stream_from "presence"
  end
end
