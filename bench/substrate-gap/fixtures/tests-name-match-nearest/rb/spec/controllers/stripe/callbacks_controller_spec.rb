require 'rails_helper'

RSpec.describe Stripe::CallbacksController, type: :request do
  describe 'GET #show' do
    it 'returns ok' do
      get '/stripe/callback'
      expect(response).to have_http_status(:ok)
    end
  end
end
