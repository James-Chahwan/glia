class ReportsController < ApplicationController
  def active
    @users = User.where(active: true)
    render json: @users
  end
end
