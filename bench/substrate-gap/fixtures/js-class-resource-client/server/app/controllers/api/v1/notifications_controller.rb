class Api::V1::NotificationsController < ApplicationController
  def index
    render json: []
  end

  def unread
    head :ok
  end

  def destroy
    head :ok
  end
end
