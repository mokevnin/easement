require_relative "../domain/order"
require_relative "../infra/order_repository"

module Shop
  module Web
    class OrderController
      def initialize
        @repo = Shop::Infra::OrderRepository.new
        @order = Shop::Domain::Order.new
      end
    end
  end
end
