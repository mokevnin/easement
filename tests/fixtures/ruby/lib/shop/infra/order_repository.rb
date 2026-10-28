require "set"
require_relative "../domain/order"

module Shop
  module Infra
    class OrderRepository
      def initialize
        @orders = Set.new
        @orders << Shop::Domain::Order.new
      end
    end
  end
end
