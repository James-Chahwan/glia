require 'swagger_helper'

RSpec.describe 'users', type: :request do
  path '/users/{id}' do
    get 'Retrieves a user' do
      operationId 'getUser'
      produces 'application/json'
      parameter name: :id, in: :path, type: :string

      response '200', 'user found' do
        let(:id) { '1' }
        run_test!
      end

      response '404', 'not found' do
        let(:id) { 'missing' }
        run_test!
      end
    end
  end
end
