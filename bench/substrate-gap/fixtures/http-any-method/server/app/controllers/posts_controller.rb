class PostsController < ApplicationController
  def index
    @posts = []
  end

  def create
    head :created
  end
end
