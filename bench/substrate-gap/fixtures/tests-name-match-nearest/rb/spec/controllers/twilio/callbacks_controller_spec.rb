require 'rails_helper'

RSpec.describe Twilio::CallbackController, type: :request do
  describe 'POST #create' do
    it 'accepts a twilio callback' do
      post '/twilio/callback'
      expect(response).to have_http_status(:ok)
    end
  end
end
