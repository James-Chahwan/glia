class Api::V1::ProfilesController < ApplicationController
  def show
    render json: {}
  end

  def update
    head :ok
  end
end
